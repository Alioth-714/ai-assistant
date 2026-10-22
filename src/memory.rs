use crate::error::{AppError, Result};
use crate::util::now_ms;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const MAX_MESSAGES: usize = 80;
const MAX_MESSAGE_CHARS: usize = 20_000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionMessage {
    pub role: String,
    pub content: String,
    pub ts: u64,
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct SessionFile {
    #[serde(default)]
    role: String,
    #[serde(default)]
    updated_at: u64,
    #[serde(default)]
    messages: Vec<SessionMessage>,
}

pub struct SessionStore {
    root: PathBuf,
}

impl SessionStore {
    pub fn new(data_root: &Path) -> Self {
        Self {
            root: data_root.join("memory"),
        }
    }

    fn path(&self, role: &str) -> PathBuf {
        self.root.join(role).join("session.json")
    }

    pub fn load(&self, role: &str) -> Vec<SessionMessage> {
        if !crate::roles::is_valid(role) {
            return Vec::new();
        }
        let path = self.path(role);
        let Ok(text) = std::fs::read_to_string(&path) else {
            return Vec::new();
        };
        match serde_json::from_str::<SessionFile>(&text) {
            Ok(file) => file.messages,
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "memory session parse failed");
                Vec::new()
            }
        }
    }

    pub fn append(&self, role: &str, user: &str, assistant: &str) -> Result<()> {
        if !crate::roles::is_valid(role) {
            return Err(AppError::InvalidInput(format!("未知角色：{role}")));
        }

        let path = self.path(role);
        let mut file = std::fs::read_to_string(&path)
            .ok()
            .and_then(|text| serde_json::from_str::<SessionFile>(&text).ok())
            .unwrap_or_default();
        let ts = now_ms();

        file.role = role.to_string();
        file.updated_at = ts;
        file.messages.push(SessionMessage {
            role: "user".to_string(),
            content: truncate(user, MAX_MESSAGE_CHARS),
            ts,
        });
        file.messages.push(SessionMessage {
            role: "assistant".to_string(),
            content: truncate(assistant, MAX_MESSAGE_CHARS),
            ts,
        });

        if file.messages.len() > MAX_MESSAGES {
            let drain_to = file.messages.len() - MAX_MESSAGES;
            file.messages.drain(0..drain_to);
        }

        self.write(&path, &file)
    }

    pub fn clear(&self, role: &str) -> Result<()> {
        if !crate::roles::is_valid(role) {
            return Err(AppError::InvalidInput(format!("未知角色：{role}")));
        }
        let file = SessionFile {
            role: role.to_string(),
            updated_at: now_ms(),
            messages: Vec::new(),
        };
        self.write(&self.path(role), &file)
    }

    fn write(&self, path: &Path, file: &SessionFile) -> Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let text = serde_json::to_string_pretty(file)?;
        let temp = path.with_extension("json.tmp");
        std::fs::write(&temp, text)?;
        if path.exists() {
            std::fs::remove_file(path)?;
        }
        std::fs::rename(&temp, path)?;
        Ok(())
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}
