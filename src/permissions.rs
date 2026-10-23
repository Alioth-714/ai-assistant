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
    #[serde(rename = "requestId")]
    pub id: String,
    pub tool: String,
    pub level: String,
    #[serde(rename = "target")]
    pub path: String,
    pub preview: String,
    pub agent: String,
}

impl PermissionRequest {
    pub fn write(path: impl Into<String>, preview: impl Into<String>) -> Self {
        Self::write_by("coordinator", path, preview)
    }

    pub fn write_by(
        agent: impl Into<String>,
        path: impl Into<String>,
        preview: impl Into<String>,
    ) -> Self {
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
            agent: agent.into(),
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

#[cfg(test)]
mod tests {
    use super::PermissionRequest;

    #[test]
    fn serializes_frontend_contract_fields() {
        let request = PermissionRequest::write("data/workspace/demo.txt", "- old\n+ new");
        let value = serde_json::to_value(&request).expect("serialize permission request");

        assert!(value.get("requestId").is_some());
        assert!(value.get("target").is_some());
        assert_eq!(
            value.get("agent").and_then(|item| item.as_str()),
            Some("coordinator")
        );
        assert_eq!(
            value.get("level").and_then(|item| item.as_str()),
            Some("L2")
        );
    }
}
