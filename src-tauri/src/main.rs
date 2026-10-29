#![windows_subsystem = "windows"]

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
fn delete_role(state: State<'_, Arc<Runtime>>, role_id: String) -> Result<(), String> {
    tracing::info!(role = %role_id, "frontend requested delete_role");
    state.delete_role(&role_id).map_err(|err| err.to_string())
}

/// 按角色名生成专业提示词（本地 7B；失败不覆盖用户已填内容）
#[tauri::command]
async fn generate_role_prompt(
    state: State<'_, Arc<Runtime>>,
    name: String,
    desc: Option<String>,
) -> Result<ai_assistant::roles::GeneratedRolePrompt, String> {
    tracing::info!(role = %name, "frontend requested generate_role_prompt");
    state
        .generate_role_prompt(&name, desc.as_deref().unwrap_or(""))
        .await
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
async fn save_provider(
    state: State<'_, Arc<Runtime>>,
    provider: ProviderInput,
) -> Result<ProviderView, String> {
    tracing::info!(provider = %provider.id, "frontend requested save_provider");
    state
        .save_provider(provider)
        .await
        .map_err(|err| err.to_string())
}

/// 清除某个 Provider 的密钥（保留 Provider 配置）
#[tauri::command]
fn clear_provider_key(
    state: State<'_, Arc<Runtime>>,
    provider_id: String,
) -> Result<ProviderView, String> {
    tracing::info!(provider = %provider_id, "frontend requested clear_provider_key");
    state
        .clear_provider_key(&provider_id)
        .map_err(|err| err.to_string())
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

/// 会话云端用量（次数 / 1 小时窗口）
#[tauri::command]
fn cloud_usage(
    state: State<'_, Arc<Runtime>>,
    session_id: String,
) -> ai_assistant::runtime::CloudUsage {
    state.cloud_usage_of(&session_id)
}

/// 本次运行内"不再询问"云端频次提示
#[tauri::command]
fn set_cloud_prompt_suppressed(state: State<'_, Arc<Runtime>>, value: bool) {
    state.set_cloud_prompt_suppressed(value);
}

/// 出网审计（决策 33）：最近的云端发送 / 拦截记录
#[tauri::command]
fn cloud_audit(state: State<'_, Arc<Runtime>>, limit: Option<usize>) -> Vec<serde_json::Value> {
    ai_assistant::audit::recent(&state.data_root, "cloud_egress", limit.unwrap_or(20))
}

/// 撤销尚未生效的数据目录切换
#[tauri::command]
fn abort_data_switch(state: State<'_, Arc<Runtime>>) -> Result<String, String> {
    tracing::info!("frontend requested abort_data_switch");
    state
        .abort_data_switch()
        .map(|path| path.display().to_string())
        .map_err(|err| err.to_string())
}

/// 切换到新的空数据目录（不迁移旧数据）
#[tauri::command]
async fn set_data_root(
    state: State<'_, Arc<Runtime>>,
    path: String,
) -> Result<ai_assistant::data::MigrationReport, String> {
    tracing::info!(target = %path, "frontend requested set_data_root (no migrate)");
    state
        .set_data_root(&path, false)
        .map_err(|err| err.to_string())
}

/// 切换到新的空数据目录，并复制现有数据（复制完成才返回，前端随后显示关闭倒计时）
#[tauri::command]
async fn migrate_data_root(
    state: State<'_, Arc<Runtime>>,
    path: String,
) -> Result<ai_assistant::data::MigrationReport, String> {
    tracing::info!(target = %path, "frontend requested migrate_data_root");
    state
        .set_data_root(&path, true)
        .map_err(|err| err.to_string())
}

/// 系统原生文件夹选择框（Win32 SHBrowseForFolder，零额外依赖）
#[cfg(windows)]
mod folder_picker {
    use std::ffi::c_void;
    use std::ptr;

    #[repr(C)]
    struct BrowseInfoW {
        hwnd_owner: *mut c_void,
        pidl_root: *mut c_void,
        psz_display_name: *mut u16,
        lpsz_title: *const u16,
        ul_flags: u32,
        lpfn: *mut c_void,
        l_param: isize,
        i_image: i32,
    }

    // BIF_RETURNONLYFSDIRS | BIF_NEWDIALOGSTYLE | BIF_EDITBOX
    const FLAGS: u32 = 0x0000_0001 | 0x0000_0040 | 0x0000_0010;
    const MAX_PATH: usize = 260;
    const COINIT_APARTMENTTHREADED: u32 = 0x2;

    #[link(name = "shell32")]
    extern "system" {
        fn SHBrowseForFolderW(info: *const BrowseInfoW) -> *mut c_void;
        fn SHGetPathFromIDListW(pidl: *const c_void, path: *mut u16) -> i32;
    }

    #[link(name = "ole32")]
    extern "system" {
        fn CoInitializeEx(reserved: *mut c_void, coinit: u32) -> i32;
        fn CoUninitialize();
        fn CoTaskMemFree(pointer: *mut c_void);
    }

    pub fn pick_folder(title: &str) -> Option<String> {
        let title_wide: Vec<u16> = title.encode_utf16().chain(std::iter::once(0)).collect();
        let mut display = vec![0u16; MAX_PATH];
        let info = BrowseInfoW {
            hwnd_owner: ptr::null_mut(),
            pidl_root: ptr::null_mut(),
            psz_display_name: display.as_mut_ptr(),
            lpsz_title: title_wide.as_ptr(),
            ul_flags: FLAGS,
            lpfn: ptr::null_mut(),
            l_param: 0,
            i_image: 0,
        };

        unsafe {
            let hr = CoInitializeEx(ptr::null_mut(), COINIT_APARTMENTTHREADED);
            let pidl = SHBrowseForFolderW(&info);
            let picked = if pidl.is_null() {
                None
            } else {
                let mut buffer = vec![0u16; MAX_PATH * 4];
                let ok = SHGetPathFromIDListW(pidl, buffer.as_mut_ptr()) != 0;
                CoTaskMemFree(pidl);
                if ok {
                    let len = buffer.iter().position(|&ch| ch == 0).unwrap_or(0);
                    Some(String::from_utf16_lossy(&buffer[..len]))
                } else {
                    None
                }
            };
            if hr >= 0 {
                CoUninitialize();
            }
            picked
        }
    }
}

#[cfg(not(windows))]
mod folder_picker {
    pub fn pick_folder(_title: &str) -> Option<String> {
        None
    }
}

/// 打开系统原生文件夹选择框，返回用户选择的目录（取消返回 None）
#[tauri::command]
async fn pick_data_dir() -> Result<Option<String>, String> {
    let picked = folder_picker::pick_folder("选择新的数据目录（必须是空文件夹）");
    tracing::info!(picked = ?picked, "frontend requested pick_data_dir");
    Ok(picked)
}

/// 前端诊断日志（写到 app.log，便于排查前端异常）
#[tauri::command]
fn ui_log(message: String) {
    tracing::info!(target: "ui", message = %message, "frontend event");
}

/// 关闭应用（数据目录切换并复制完成后使用，由用户手动重新打开）
#[tauri::command]
fn close_app(app: tauri::AppHandle) {
    tracing::info!("close requested by frontend (data root switched)");
    app.exit(0);
}

/// 立即重启应用（保留命令，前端目前不用）
///
/// 用 `ping` 做延迟（`timeout` 在无控制台的进程里会因 stdin 重定向而失败），
/// 延迟 2 秒再拉起新实例，避免新旧进程争抢 WebView2 用户数据目录。
#[tauri::command]
fn restart_app(app: tauri::AppHandle) -> Result<(), String> {
    let exe = std::env::current_exe().map_err(|err| format!("无法定位程序路径：{err}"))?;
    let command = format!(
        "ping -n 3 127.0.0.1 >nul & start \"\" \"{}\"",
        exe.display()
    );
    tracing::info!(exe = %exe.display(), "restart requested by frontend");
    let spawned = std::process::Command::new("cmd")
        .args(["/C", command.as_str()])
        .stdin(std::process::Stdio::null())
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn();
    match spawned {
        Ok(_) => {
            app.exit(0);
            Ok(())
        }
        Err(err) => Err(format!(
            "自动重启失败（{err}），请手动关闭后重新打开应用；数据目录已经切换，重启后生效"
        )),
    }
}

/// 自动获取 Provider 的模型列表（决策 33 云端对接）
#[tauri::command]
async fn fetch_provider_models(
    state: State<'_, Arc<Runtime>>,
    base_url: String,
    api_key: Option<String>,
    provider_id: Option<String>,
) -> Result<Vec<String>, String> {
    tracing::info!(base_url = %base_url, has_key = api_key.is_some(), "frontend requested fetch_provider_models");
    match state
        .fetch_provider_models(&base_url, api_key.as_deref(), provider_id.as_deref())
        .await
    {
        Ok(models) => {
            tracing::info!(count = models.len(), "fetch_provider_models ok");
            Ok(models)
        }
        Err(err) => {
            tracing::warn!(error = %err, "fetch_provider_models failed");
            Err(err.to_string())
        }
    }
}

/// 预设文件版本号（用于提示是否有更新）
#[tauri::command]
fn presets_version(state: State<'_, Arc<Runtime>>) -> u32 {
    ai_assistant::providers::presets_version(&state.app_root)
}

/// Provider 预设（读 config/providers.json，随版本更新）
#[tauri::command]
fn provider_presets(
    state: State<'_, Arc<Runtime>>,
) -> Vec<ai_assistant::providers::ProviderPreset> {
    ai_assistant::providers::presets_from(&state.app_root)
}

/// 设置页模型选项（按档位过滤、排除 embedding、只列已登记模型）
#[tauri::command]
async fn model_options(
    state: State<'_, Arc<Runtime>>,
) -> Result<Vec<ai_assistant::runtime::ModelOption>, String> {
    Ok(state.model_options().await)
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
    state
        .import_gguf(request)
        .await
        .map_err(|err| err.to_string())
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

/// 控制台工具：正常情况下不显示黑框，只有 `--console` 或启动失败时才申请控制台。
#[cfg(windows)]
mod console {
    use std::ffi::c_void;
    use std::ptr;

    const STD_INPUT_HANDLE: u32 = -10i32 as u32;
    const STD_OUTPUT_HANDLE: u32 = -11i32 as u32;
    const MB_ICONERROR: u32 = 0x0000_0010;

    #[link(name = "kernel32")]
    extern "system" {
        fn AllocConsole() -> i32;
        fn GetStdHandle(kind: u32) -> *mut c_void;
        fn SetConsoleOutputCP(code_page: u32) -> i32;
        fn SetConsoleTitleW(title: *const u16) -> i32;
        fn WriteFile(
            handle: *mut c_void,
            buffer: *const u8,
            len: u32,
            written: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
        fn ReadFile(
            handle: *mut c_void,
            buffer: *mut u8,
            len: u32,
            read: *mut u32,
            overlapped: *mut c_void,
        ) -> i32;
    }

    #[link(name = "user32")]
    extern "system" {
        fn MessageBoxW(hwnd: *mut c_void, text: *const u16, caption: *const u16, utype: u32)
            -> i32;
    }

    pub fn alloc() {
        unsafe {
            AllocConsole();
            SetConsoleOutputCP(65001); // UTF-8，避免中文乱码
            let title = to_wide("小明同学 · 控制台");
            SetConsoleTitleW(title.as_ptr());
        }
    }

    fn write(text: &str) {
        let handle = unsafe { GetStdHandle(STD_OUTPUT_HANDLE) };
        if handle.is_null() {
            return;
        }
        let mut written = 0u32;
        unsafe {
            WriteFile(
                handle,
                text.as_ptr(),
                text.len() as u32,
                &mut written,
                ptr::null_mut(),
            );
        }
    }

    /// 启动失败：弹窗 + 控制台输出，等用户回车再退出
    pub fn show_splash_error(title: &str, message: &str) -> ! {
        alloc();
        write(&format!(
            "\n==== {title} ====\n{message}\n\n详细日志见 data\\logs\\app.*.log\n\n按回车键退出…\n"
        ));
        let caption = to_wide(title);
        let body = to_wide(&format!("{message}\n\n详细日志见 data\\logs\\app.*.log"));
        unsafe {
            MessageBoxW(
                ptr::null_mut(),
                body.as_ptr(),
                caption.as_ptr(),
                MB_ICONERROR,
            );
        }
        let handle = unsafe { GetStdHandle(STD_INPUT_HANDLE) };
        if !handle.is_null() {
            let mut buffer = [0u8; 1];
            let mut read = 0u32;
            unsafe {
                ReadFile(handle, buffer.as_mut_ptr(), 1, &mut read, ptr::null_mut());
            }
        }
        std::process::exit(1);
    }

    fn to_wide(text: &str) -> Vec<u16> {
        text.encode_utf16().chain(std::iter::once(0)).collect()
    }
}

#[cfg(not(windows))]
mod console {
    pub fn alloc() {}

    pub fn show_splash_error(title: &str, message: &str) -> ! {
        eprintln!("\n==== {title} ====\n{message}\n");
        std::process::exit(1);
    }
}

/// 启动失败统一出口：写日志 + 弹窗 + 控制台，然后退出
fn fail_startup(message: &str) -> ! {
    tracing::error!(error = %message, "startup failed");
    // 非阻塞日志在 exit 时可能丢，这里同步落盘一份，方便排查
    let mut targets = vec![std::env::temp_dir().join("ai-assistant-startup-error.log")];
    if let Ok(root) = std::env::var("AI_ASSISTANT_DATA") {
        if !root.trim().is_empty() {
            targets.push(std::path::PathBuf::from(root).join("startup-error.log"));
        }
    }
    let stamp = format!("startup failed: {message}\n");
    for path in targets {
        if let Some(parent) = path.parent() {
            let _ = std::fs::create_dir_all(parent);
        }
        let _ = std::fs::write(&path, &stamp);
    }
    console::show_splash_error("小明同学启动失败", message);
}

fn main() {
    // 仅排查时手动开控制台：ai-assistant-app.exe --console
    if std::env::args().any(|arg| arg == "--console") {
        std::env::set_var("AI_ASSISTANT_CONSOLE", "1");
        console::alloc();
    }

    let runtime = match bootstrap() {
        Ok(runtime) => Arc::new(runtime),
        Err(err) => fail_startup(&format!("{err}")),
    };

    tracing::info!("starting tauri window");

    let result = tauri::Builder::default()
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
            cloud_usage,
            set_cloud_prompt_suppressed,
            cloud_audit,
            set_data_root,
            migrate_data_root,
            abort_data_switch,
            pick_data_dir,
            restart_app,
            close_app,
            ui_log,
            fetch_provider_models,
            provider_presets,
            presets_version,
            model_options,
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
            generate_role_prompt,
            list_local_models,
            list_providers,
            save_provider,
            clear_provider_key,
            delete_provider,
            test_provider,
            prepare_external,
            grant_file_export,
            revoke_file_export,
            send_external_message
        ])
        .run(tauri::generate_context!());

    if let Err(err) = result {
        fail_startup(&format!("窗口启动失败：{err}"));
    }
}
