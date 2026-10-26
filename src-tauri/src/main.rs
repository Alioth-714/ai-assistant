#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ai_assistant::tasks::TaskStore;
use ai_assistant::{
    bootstrap, AppStatus, ChatReply, EnvironmentReport, ExportGate, ExternalChatOutcome,
    LongTermFact, PermissionPrompt, PermissionRequest, ProgressSink, ProviderInput, ProviderTest,
    ProviderView, RoleSummary, RouterDecision, Runtime, SaveSettingsResult, SessionDetail,
    SessionMessage, SessionSummary, SettingsView, TaskProgress, TaskSummary, UserSettings,
};
use std::collections::{HashMap, HashSet};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, State};
use tokio::sync::oneshot;

const PERMISSION_TIMEOUT: Duration = Duration::from_secs(120);

struct PendingRequest {
    sender: oneshot::Sender<bool>,
    created_at: Instant,
    path: String,
}

type PendingMap = Arc<Mutex<HashMap<String, PendingRequest>>>;
type AllowedPathList = Arc<Mutex<HashSet<String>>>;

struct PermissionState {
    pending: PendingMap,
    allowed_paths: AllowedPathList,
}

impl PermissionState {
    fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
            allowed_paths: Arc::new(Mutex::new(HashSet::new())),
        }
    }

    fn shared_pending(&self) -> PendingMap {
        self.pending.clone()
    }

    fn shared_allowed_paths(&self) -> AllowedPathList {
        self.allowed_paths.clone()
    }
}

struct TauriPrompt {
    app: AppHandle,
    pending: PendingMap,
    allowed_paths: AllowedPathList,
}

struct TauriProgress {
    app: AppHandle,
}

impl ProgressSink for TauriProgress {
    fn emit(&self, progress: TaskProgress) {
        if let Err(err) = self.app.emit_all("task_progress", &progress) {
            tracing::warn!(stage = %progress.stage, error = %err, "emit task progress failed");
        }
    }
}

impl PermissionPrompt for TauriPrompt {
    fn request_write(&self, request: PermissionRequest, responder: oneshot::Sender<bool>) {
        let already_allowed = self
            .allowed_paths
            .lock()
            .expect("allowed paths lock")
            .contains(&request.path);
        if already_allowed {
            tracing::info!(
                id = %request.id,
                path = %request.path,
                "permission auto-approved for session path"
            );
            let _ = responder.send(true);
            return;
        }

        {
            let mut pending = self.pending.lock().expect("pending permission lock");
            pending.insert(
                request.id.clone(),
                PendingRequest {
                    sender: responder,
                    created_at: Instant::now(),
                    path: request.path.clone(),
                },
            );
        }

        tracing::info!(
            id = %request.id,
            path = %request.path,
            "permission confirmation requested"
        );

        if let Err(err) = self.app.emit_all("permission_request", &request) {
            tracing::warn!(id = %request.id, error = %err, "emit permission request failed");
            if let Some(entry) = self
                .pending
                .lock()
                .expect("pending permission lock")
                .remove(&request.id)
            {
                let _ = entry.sender.send(false);
            }
        }
    }
}

#[tauri::command]
async fn get_status(
    state: State<'_, Arc<Runtime>>,
    permissions: State<'_, PermissionState>,
) -> Result<AppStatus, String> {
    tracing::info!("frontend requested get_status");
    let mut status = state.status().await;

    let mut pending = permissions.pending.lock().map_err(|err| err.to_string())?;
    pending.retain(|_, entry| entry.created_at.elapsed() < PERMISSION_TIMEOUT);
    status.pending_confirm = pending.len() as u32;
    drop(pending);

    Ok(status)
}

#[tauri::command]
async fn send_message(
    app: AppHandle,
    state: State<'_, Arc<Runtime>>,
    permissions: State<'_, PermissionState>,
    role: String,
    content: String,
    session_id: Option<String>,
) -> Result<ChatReply, String> {
    tracing::info!(
        role = %role,
        chars = content.chars().count(),
        session_id = ?session_id,
        "frontend requested send_message"
    );

    let progress = TauriProgress { app: app.clone() };
    let prompt = TauriPrompt {
        app,
        pending: permissions.shared_pending(),
        allowed_paths: permissions.shared_allowed_paths(),
    };

    match state
        .chat_with_progress(
            &role,
            &content,
            Some(&prompt),
            Some(&progress),
            session_id.as_deref(),
        )
        .await
    {
        Ok(reply) => Ok(reply),
        Err(err) => {
            tracing::warn!(error = %err, "send_message failed");
            Err(err.to_string())
        }
    }
}

#[tauri::command]
fn confirm_permission(
    permissions: State<'_, PermissionState>,
    request_id: String,
    action: String,
) -> Result<(), String> {
    let action = action.trim().to_ascii_lowercase().replace('-', "_");
    if !matches!(action.as_str(), "allow" | "allow_file" | "deny") {
        return Err(format!("未知权限操作：{action}"));
    }

    let entry = permissions
        .pending
        .lock()
        .map_err(|err| err.to_string())?
        .remove(&request_id)
        .ok_or_else(|| "权限请求不存在或已超时".to_string())?;

    if action == "allow_file" {
        permissions
            .allowed_paths
            .lock()
            .map_err(|err| err.to_string())?
            .insert(entry.path.clone());
    }

    tracing::info!(
        id = %request_id,
        action = %action,
        path = %entry.path,
        "permission resolved"
    );
    entry
        .sender
        .send(action != "deny")
        .map_err(|_| "权限确认已超时".to_string())
}

#[tauri::command]
fn resolve_permission(
    permissions: State<'_, PermissionState>,
    id: String,
    approved: bool,
) -> Result<(), String> {
    let entry = permissions
        .pending
        .lock()
        .map_err(|err| err.to_string())?
        .remove(&id)
        .ok_or_else(|| "权限请求不存在或已超时".to_string())?;

    tracing::info!(id = %id, approved, "permission resolved (legacy)");
    entry
        .sender
        .send(approved)
        .map_err(|_| "权限确认已超时".to_string())
}

#[tauri::command]
fn get_history(
    state: State<'_, Arc<Runtime>>,
    role: String,
    session_id: Option<String>,
) -> Result<Vec<SessionMessage>, String> {
    Ok(state.history_for(&role, session_id.as_deref()))
}

#[tauri::command]
fn clear_history(
    state: State<'_, Arc<Runtime>>,
    role: String,
    session_id: Option<String>,
) -> Result<(), String> {
    tracing::info!(role = %role, session_id = ?session_id, "frontend requested clear_history");
    match session_id.as_deref() {
        Some(id) => state.clear_session(&role, id),
        None => state.clear_history(&role),
    }
    .map_err(|err| err.to_string())
}

#[tauri::command]
fn list_sessions(state: State<'_, Arc<Runtime>>, role: String) -> Vec<SessionSummary> {
    state.list_sessions(&role)
}

#[tauri::command]
fn create_session(
    state: State<'_, Arc<Runtime>>,
    role: String,
    title: Option<String>,
) -> Result<SessionSummary, String> {
    tracing::info!(role = %role, "frontend requested create_session");
    state
        .create_session(&role, title.as_deref())
        .map_err(|err| err.to_string())
}

#[tauri::command]
fn get_session(
    state: State<'_, Arc<Runtime>>,
    role: String,
    session_id: String,
) -> Result<SessionDetail, String> {
    state
        .session_detail(&role, &session_id)
        .map_err(|err| err.to_string())
}

#[tauri::command]
fn rename_session(
    state: State<'_, Arc<Runtime>>,
    role: String,
    session_id: String,
    title: String,
) -> Result<(), String> {
    tracing::info!(role = %role, session_id = %session_id, "frontend requested rename_session");
    state
        .rename_session(&role, &session_id, &title)
        .map_err(|err| err.to_string())
}

#[tauri::command]
fn delete_session(
    state: State<'_, Arc<Runtime>>,
    role: String,
    session_id: String,
) -> Result<(), String> {
    tracing::info!(role = %role, session_id = %session_id, "frontend requested delete_session");
    state
        .delete_session(&role, &session_id)
        .map_err(|err| err.to_string())
}

#[tauri::command]
fn get_settings(state: State<'_, Arc<Runtime>>) -> SettingsView {
    tracing::info!("frontend requested get_settings");
    state.settings_view()
}

#[tauri::command]
fn save_settings(
    state: State<'_, Arc<Runtime>>,
    settings: UserSettings,
) -> Result<SaveSettingsResult, String> {
    tracing::info!(theme = %settings.theme, "frontend requested save_settings");
    state.save_settings(settings).map_err(|err| err.to_string())
}

#[tauri::command]
fn list_roles(state: State<'_, Arc<Runtime>>) -> Vec<RoleSummary> {
    tracing::info!("frontend requested list_roles");
    state.roles()
}

#[tauri::command]
fn save_role(state: State<'_, Arc<Runtime>>, role: RoleSummary) -> Result<RoleSummary, String> {
    tracing::info!(role = %role.id, "frontend requested save_role");
    state.save_role(role).map_err(|err| err.to_string())
}

#[tauri::command]
fn delete_role(
    state: State<'_, Arc<Runtime>>,
    role_id: String,
    delete_data: bool,
) -> Result<(), String> {
    tracing::info!(role = %role_id, delete_data, "frontend requested delete_role");
    state
        .delete_role(&role_id, delete_data)
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn list_local_models(state: State<'_, Arc<Runtime>>) -> Result<Vec<String>, String> {
    tracing::info!("frontend requested list_local_models");
    state
        .list_local_models()
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
fn list_providers(state: State<'_, Arc<Runtime>>) -> Vec<ProviderView> {
    tracing::info!("frontend requested list_providers");
    state.providers_view()
}

#[tauri::command]
fn save_provider(
    state: State<'_, Arc<Runtime>>,
    provider: ProviderInput,
) -> Result<ProviderView, String> {
    tracing::info!(provider = %provider.id, "frontend requested save_provider");
    state.save_provider(provider).map_err(|err| err.to_string())
}

#[tauri::command]
fn delete_provider(state: State<'_, Arc<Runtime>>, provider_id: String) -> Result<(), String> {
    tracing::info!(provider = %provider_id, "frontend requested delete_provider");
    state
        .delete_provider(&provider_id)
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn test_provider(
    state: State<'_, Arc<Runtime>>,
    provider_id: String,
    force: Option<bool>,
) -> Result<ProviderTest, String> {
    tracing::info!(provider = %provider_id, "frontend requested test_provider");
    state
        .test_provider(&provider_id, force.unwrap_or(false))
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
fn prepare_external(
    state: State<'_, Arc<Runtime>>,
    role: String,
    session_id: Option<String>,
    provider_id: String,
) -> Result<ExportGate, String> {
    state
        .prepare_external(&role, session_id.as_deref(), &provider_id)
        .map_err(|err| err.to_string())
}

#[tauri::command]
fn grant_file_export(
    state: State<'_, Arc<Runtime>>,
    role_id: String,
    provider_id: String,
) -> Result<(), String> {
    tracing::info!(role = %role_id, provider = %provider_id, "frontend granted file export");
    state
        .grant_file_export(&role_id, &provider_id)
        .map_err(|err| err.to_string())
}

#[tauri::command]
fn revoke_file_export(state: State<'_, Arc<Runtime>>, provider_id: String) -> Result<(), String> {
    tracing::info!(provider = %provider_id, "frontend revoked file export");
    state
        .revoke_file_export(&provider_id)
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn send_external_message(
    state: State<'_, Arc<Runtime>>,
    role: String,
    content: String,
    session_id: Option<String>,
    provider_id: String,
    include_context: bool,
) -> Result<ExternalChatOutcome, String> {
    tracing::info!(
        role = %role,
        provider = %provider_id,
        include_context,
        "frontend requested send_external_message"
    );
    state
        .send_external_message(
            &role,
            &content,
            session_id.as_deref(),
            &provider_id,
            include_context,
        )
        .await
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn route_message(
    state: State<'_, Arc<Runtime>>,
    content: String,
) -> Result<RouterDecision, String> {
    tracing::info!(
        chars = content.chars().count(),
        "frontend requested route_message"
    );
    Ok(state.route(&content).await)
}

/// 能力分层路由（决策 33）：确定性判定，不调模型。前端用于「仅本地」提示与状态栏。
#[tauri::command]
fn capability_route(
    state: State<'_, Arc<Runtime>>,
    content: String,
) -> ai_assistant::capabilities::RouteDecision {
    state.route_decision(&content)
}

/// 当前能力档位与能力表（设置页展示 / 换硬件后切 profile）
#[tauri::command]
fn capability_view(state: State<'_, Arc<Runtime>>) -> ai_assistant::capabilities::Capabilities {
    state.capabilities_view()
}

/// 会话云端预算（已用 / 上限）
#[tauri::command]
fn cloud_budget(state: State<'_, Arc<Runtime>>) -> ai_assistant::egress::CloudBudget {
    state.cloud_budget()
}

/// 出网审计（决策 33）：最近的云端发送 / 拦截记录
#[tauri::command]
fn cloud_audit(state: State<'_, Arc<Runtime>>, limit: Option<usize>) -> Vec<serde_json::Value> {
    ai_assistant::audit::recent(&state.data_root, "cloud_egress", limit.unwrap_or(20))
}

/// 数据目录迁移（5A）：复制并校验后写指针文件，重启后生效
#[tauri::command]
fn set_data_root(
    state: State<'_, Arc<Runtime>>,
    path: String,
) -> Result<ai_assistant::data::MigrationReport, String> {
    tracing::info!(target = %path, "frontend requested set_data_root");
    state.set_data_root(&path).map_err(|err| err.to_string())
}

/// 自动获取 Provider 的模型列表（决策 33 云端对接）
#[tauri::command]
async fn fetch_provider_models(
    state: State<'_, Arc<Runtime>>,
    base_url: String,
    api_key: Option<String>,
    provider_id: Option<String>,
) -> Result<Vec<String>, String> {
    state
        .fetch_provider_models(&base_url, api_key.as_deref(), provider_id.as_deref())
        .await
        .map_err(|err| err.to_string())
}

/// 内置 Provider 预设（官方直连 / 聚合网关）
#[tauri::command]
fn provider_presets() -> Vec<ai_assistant::providers::ProviderPreset> {
    ai_assistant::providers::presets()
}

/// 扫描 models 目录里的 GGUF 候选
#[tauri::command]
async fn scan_models(
    state: State<'_, Arc<Runtime>>,
) -> Result<Vec<ai_assistant::model_import::GgufCandidate>, String> {
    state.scan_models().await.map_err(|err| err.to_string())
}

/// 导入一个 GGUF（生成 Modelfile + ollama create + 登记 models.json）
#[tauri::command]
async fn import_gguf(
    state: State<'_, Arc<Runtime>>,
    request: ai_assistant::model_import::ImportRequest,
) -> Result<ai_assistant::model_import::ImportResult, String> {
    tracing::info!(model = %request.model_name, "frontend requested import_gguf");
    state.import_gguf(request).await.map_err(|err| err.to_string())
}

/// 打开本地目录（models / data / logs）
#[tauri::command]
fn open_local_path(state: State<'_, Arc<Runtime>>, kind: String) -> Result<String, String> {
    state
        .open_local_path(&kind)
        .map(|path| path.display().to_string())
        .map_err(|err| err.to_string())
}

#[tauri::command]
async fn check_environment(state: State<'_, Arc<Runtime>>) -> Result<EnvironmentReport, String> {
    tracing::info!("frontend requested check_environment");
    Ok(state.environment().await)
}

#[tauri::command]
fn get_memory(state: State<'_, Arc<Runtime>>) -> Vec<LongTermFact> {
    state.memory_facts()
}

#[tauri::command]
fn list_tasks(state: State<'_, Arc<Runtime>>, limit: Option<usize>) -> Vec<TaskSummary> {
    TaskStore::new(&state.data_root).list(limit.unwrap_or(20).min(100))
}

#[tauri::command]
fn get_task(state: State<'_, Arc<Runtime>>, task_id: String) -> Option<serde_json::Value> {
    TaskStore::new(&state.data_root).get(&task_id)
}

fn main() {
    let runtime = match bootstrap() {
        Ok(runtime) => Arc::new(runtime),
        Err(err) => {
            eprintln!("启动失败：{err}");
            std::process::exit(1);
        }
    };

    tracing::info!("starting tauri window");

    tauri::Builder::default()
        .manage(runtime)
        .manage(PermissionState::new())
        .setup(|app| {
            let runtime = app.state::<Arc<Runtime>>().inner().clone();
            tauri::async_runtime::spawn(async move {
                let report = runtime.environment().await;
                tracing::info!(
                    ok = report.ok,
                    ollama_reachable = report.ollama_reachable,
                    missing = ?report.missing_models,
                    "startup environment check"
                );
                runtime.warmup_router().await;
                runtime.warmup_coordinator().await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            send_message,
            route_message,
            capability_route,
            capability_view,
            cloud_budget,
            cloud_audit,
            set_data_root,
            fetch_provider_models,
            provider_presets,
            scan_models,
            import_gguf,
            open_local_path,
            confirm_permission,
            resolve_permission,
            get_history,
            clear_history,
            list_sessions,
            create_session,
            get_session,
            rename_session,
            delete_session,
            check_environment,
            get_memory,
            list_tasks,
            get_task,
            get_settings,
            save_settings,
            list_roles,
            save_role,
            delete_role,
            list_local_models,
            list_providers,
            save_provider,
            delete_provider,
            test_provider,
            prepare_external,
            grant_file_export,
            revoke_file_export,
            send_external_message
        ])
        .run(tauri::generate_context!())
        .expect("运行 Tauri 窗口失败");
}
