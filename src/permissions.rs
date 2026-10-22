use crate::util::now_ms;
use serde::Serialize;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Duration;
use tokio::sync::oneshot;

const WRITE_CONFIRM_TIMEOUT: Duration = Duration::from_secs(120);
static REQUEST_SEQ: AtomicU64 = AtomicU64::new(1);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
pub enum PermissionLevel {
    L0,
    L1,
    L2,
    L3,
}

impl PermissionLevel {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::L0 => "L0",
            Self::L1 => "L1",
            Self::L2 => "L2",
            Self::L3 => "L3",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PermissionRequest {
    pub id: String,
    pub tool: String,
    pub level: String,
    pub path: String,
    pub preview: String,
}

impl PermissionRequest {
    pub fn write(path: impl Into<String>, preview: impl Into<String>) -> Self {
        Self {
            id: format!(
                "perm-{}-{}",
                now_ms(),
                REQUEST_SEQ.fetch_add(1, Ordering::Relaxed)
            ),
            tool: "write_file".to_string(),
            level: PermissionLevel::L2.as_str().to_string(),
            path: path.into(),
            preview: preview.into(),
        }
    }
}

pub trait PermissionPrompt: Send + Sync {
    fn request_write(&self, request: PermissionRequest, responder: oneshot::Sender<bool>);
}

pub struct AutoApprove;

impl PermissionPrompt for AutoApprove {
    fn request_write(&self, _request: PermissionRequest, responder: oneshot::Sender<bool>) {
        let _ = responder.send(true);
    }
}

pub struct AutoDeny;

impl PermissionPrompt for AutoDeny {
    fn request_write(&self, _request: PermissionRequest, responder: oneshot::Sender<bool>) {
        let _ = responder.send(false);
    }
}

pub async fn confirm_write(
    prompt: Option<&dyn PermissionPrompt>,
    request: PermissionRequest,
) -> bool {
    let Some(prompt) = prompt else {
        return false;
    };

    let (sender, receiver) = oneshot::channel();
    prompt.request_write(request, sender);

    match tokio::time::timeout(WRITE_CONFIRM_TIMEOUT, receiver).await {
        Ok(Ok(approved)) => approved,
        _ => false,
    }
}
