use crate::error::Result;
use std::path::{Path, PathBuf};

pub const LAYOUT: &[&str] = &[
    "config",
    "knowledge",
    "knowledge/shared",
    "knowledge/general",
    "memory",
    "memory/shared",
    "memory/dev",
    "memory/general",
    "workspace",
    "workspace/.locks",
    "backups",
    "cache",
    "cache/external",
    "cache/role_contexts",
    "cache/model_responses",
    "logs",
    "roles",
];

pub fn root(app_root: &Path) -> PathBuf {
    if let Ok(value) = std::env::var("AI_ASSISTANT_DATA") {
        if !value.trim().is_empty() {
            return PathBuf::from(value);
        }
    }
    if cfg!(debug_assertions) {
        return app_root.join("data");
    }

    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        if !local.trim().is_empty() {
            return PathBuf::from(local).join("ai-assistant-data");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("ai-assistant")
                .join("data");
        }
    }
    app_root.join("data")
}

pub fn ensure_layout(root: &Path) -> Result<()> {
    std::fs::create_dir_all(root)?;
    for dir in LAYOUT {
        std::fs::create_dir_all(root.join(dir))?;
    }
    Ok(())
}
