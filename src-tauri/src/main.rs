#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

use ai_assistant::{
    bootstrap, AppStatus, ChatReply, PermissionPrompt, PermissionRequest, RouterDecision, Runtime,
    SessionMessage,
};
use std::collections::HashMap;
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};
use tauri::{AppHandle, Manager, State};
use tokio::sync::oneshot;

const PERMISSION_TIMEOUT: Duration = Duration::from_secs(120);

type PendingMap = Arc<Mutex<HashMap<String, (oneshot::Sender<bool>, Instant)>>>;

struct PermissionState {
    pending: PendingMap,
}

impl PermissionState {
    fn new() -> Self {
        Self {
            pending: Arc::new(Mutex::new(HashMap::new())),
        }
    }

    fn shared(&self) -> PendingMap {
        self.pending.clone()
    }
}

struct TauriPrompt {
    app: AppHandle,
    pending: PendingMap,
}

impl PermissionPrompt for TauriPrompt {
    fn request_write(&self, request: PermissionRequest, responder: oneshot::Sender<bool>) {
        {
            let mut pending = self.pending.lock().expect("pending permission lock");
            pending.insert(request.id.clone(), (responder, Instant::now()));
        }

        tracing::info!(
            id = %request.id,
            path = %request.path,
            "permission confirmation requested"
        );

        if let Err(err) = self.app.emit_all("permission-request", &request) {
            tracing::warn!(id = %request.id, error = %err, "emit permission request failed");
            if let Some((sender, _)) = self
                .pending
                .lock()
                .expect("pending permission lock")
                .remove(&request.id)
            {
                let _ = sender.send(false);
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
    pending.retain(|_, (_, created_at)| created_at.elapsed() < PERMISSION_TIMEOUT);
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

    let prompt = TauriPrompt {
        app,
        pending: permissions.shared(),
    };

    match state.chat(&role, &content, Some(&prompt)).await {
        Ok(reply) => Ok(reply),
        Err(err) => {
            tracing::warn!(error = %err, "send_message failed");
            Err(err.to_string())
        }
    }
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
        .remove(&id);

    match entry {
        Some((sender, _)) => {
            tracing::info!(id = %id, approved, "permission resolved");
            sender
                .send(approved)
                .map_err(|_| "权限确认已超时".to_string())
        }
        None => Err("权限请求不存在或已超时".to_string()),
    }
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
                runtime.warmup_router().await;
                runtime.warmup_coordinator().await;
            });
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            get_status,
            send_message,
            route_message,
            resolve_permission,
            get_history,
            clear_history
        ])
        .run(tauri::generate_context!())
        .expect("运行 Tauri 窗口失败");
}
