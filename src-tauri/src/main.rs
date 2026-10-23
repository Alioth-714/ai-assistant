#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ai_assistant::{
    bootstrap, AppStatus, ChatReply, EnvironmentReport, LongTermFact, PermissionPrompt,
    PermissionRequest, ProgressSink, RouterDecision, Runtime, SessionMessage, TaskProgress,
    TaskSummary,
};
use ai_assistant::tasks::TaskStore;
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
) -> Result<ChatReply, String> {
    tracing::info!(
        role = %role,
        chars = content.chars().count(),
        "frontend requested send_message"
    );

    let progress = TauriProgress { app: app.clone() };
    let prompt = TauriPrompt {
        app,
        pending: permissions.shared_pending(),
        allowed_paths: permissions.shared_allowed_paths(),
    };

    match state
        .chat_with_progress(&role, &content, Some(&prompt), Some(&progress))
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
fn get_history(state: State<'_, Arc<Runtime>>, role: String) -> Result<Vec<SessionMessage>, String> {
    Ok(state.history(&role))
}

#[tauri::command]
fn clear_history(state: State<'_, Arc<Runtime>>, role: String) -> Result<(), String> {
    tracing::info!(role = %role, "frontend requested clear_history");
    state.clear_history(&role).map_err(|err| err.to_string())
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

#[tauri::command]
async fn check_environment(
    state: State<'_, Arc<Runtime>>,
) -> Result<EnvironmentReport, String> {
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
            confirm_permission,
            resolve_permission,
            get_history,
            clear_history,
            check_environment,
            get_memory,
            list_tasks,
            get_task
        ])
        .run(tauri::generate_context!())
        .expect("运行 Tauri 窗口失败");
}
