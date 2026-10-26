use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

const EMBEDDED_AGENT_PERSONA: &str = include_str!("../docs/agent.md");

pub const DEFAULT_ROLE: &str = "dev";
pub const ROLE_IDS: [&str; 3] = ["dev", "general", "custom-role"];

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RoleSummary {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub desc: String,
    #[serde(default)]
    pub system_prompt: String,
    #[serde(default = "default_model_preference")]
    pub model_preference: String,
    #[serde(default)]
    pub builtin: bool,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_model_preference() -> String {
    "heavy".to_string()
}

fn default_true() -> bool {
    true
}

fn builtin_roles() -> Vec<RoleSummary> {
    vec![
        RoleSummary {
            id: "dev".to_string(),
            name: "开发者".to_string(),
            desc: "代码、调试、工程实现".to_string(),
            system_prompt: String::new(),
            model_preference: "heavy".to_string(),
            builtin: true,
            enabled: true,
        },
        RoleSummary {
            id: "general".to_string(),
            name: "通用用户".to_string(),
            desc: "日常问答、写作、资料整理".to_string(),
            system_prompt: String::new(),
            model_preference: "heavy".to_string(),
            builtin: true,
            enabled: true,
        },
        RoleSummary {
            id: "custom-role".to_string(),
            name: "垂直角色".to_string(),
            desc: "自定义领域专家".to_string(),
            system_prompt: String::new(),
            model_preference: "heavy".to_string(),
            builtin: true,
            enabled: true,
        },
    ]
}

pub fn is_valid(role: &str) -> bool {
    ROLE_IDS.contains(&role)
}

pub fn is_valid_at(role: &str, data_root: &Path) -> bool {
    is_valid(role) || load_custom(role, data_root).is_some()
}

pub fn display_name(role: &str) -> &'static str {
    match role {
        "dev" => "开发者",
        "general" => "通用用户",
        "custom-role" => "垂直角色",
        _ => "通用用户",
    }
}

pub fn list(data_root: &Path) -> Vec<RoleSummary> {
    let mut roles = builtin_roles();
    let dir = roles_dir(data_root);
    if let Ok(entries) = std::fs::read_dir(dir) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(mut role) = serde_json::from_str::<RoleSummary>(&text) else {
                continue;
            };
            role.builtin = false;
            role.id = path
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_string();
            if !is_safe_id(&role.id) || roles.iter().any(|existing| existing.id == role.id) {
                continue;
            }
            roles.push(role);
        }
    }
    roles
}

pub fn save_custom(data_root: &Path, mut role: RoleSummary) -> Result<RoleSummary> {
    role.id = role.id.trim().to_string();
    role.name = role.name.trim().to_string();
    role.desc = role.desc.trim().to_string();
    role.system_prompt = role.system_prompt.trim().to_string();
    role.model_preference = role.model_preference.trim().to_string();

    if !is_safe_id(&role.id) {
        return Err(AppError::InvalidInput(
            "角色 ID 只能包含小写字母、数字和短横线，长度 1-64".to_string(),
        ));
    }
    if is_valid(&role.id) {
        return Err(AppError::InvalidInput("内置角色不可修改".to_string()));
    }
    if role.name.is_empty() {
        return Err(AppError::InvalidInput("角色名称不能为空".to_string()));
    }
    if role.system_prompt.is_empty() {
        return Err(AppError::InvalidInput("角色系统提示词不能为空".to_string()));
    }
    if role.model_preference.is_empty() {
        role.model_preference = default_model_preference();
    }

    role.builtin = false;
    role.enabled = true;

    let dir = roles_dir(data_root);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{}.json", role.id));
    let text = serde_json::to_string_pretty(&role)?;
    std::fs::write(&path, text)?;
    tracing::info!(role = %role.id, path = %path.display(), "saved custom role");
    Ok(role)
}

pub fn delete_custom(data_root: &Path, id: &str, delete_data: bool) -> Result<()> {
    if is_valid(id) {
        return Err(AppError::InvalidInput("内置角色不可删除".to_string()));
    }
    if !is_safe_id(id) {
        return Err(AppError::InvalidInput("角色 ID 非法".to_string()));
    }

    let memory_dir = data_root.join("memory").join(id);
    if memory_dir.exists() {
        if delete_data {
            std::fs::remove_dir_all(&memory_dir)?;
        } else {
            let archive_root = data_root.join("memory").join("_deleted");
            std::fs::create_dir_all(&archive_root)?;
            let archive = archive_root.join(format!("{id}-{}", now_ms()));
            std::fs::rename(&memory_dir, &archive)?;
            tracing::info!(role = %id, path = %archive.display(), "archived role data");
        }
    }

    let path = roles_dir(data_root).join(format!("{id}.json"));
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    tracing::info!(role = %id, delete_data, "deleted custom role");
    Ok(())
}

pub fn system_prompt(role: &str, app_root: &Path, data_root: &Path) -> String {
    if role == "dev" {
        let persona_path = app_root.join("docs").join("agent.md");
        let text = std::fs::read_to_string(&persona_path)
            .unwrap_or_else(|_| EMBEDDED_AGENT_PERSONA.to_string());
        let excerpt: String = text.chars().take(1200).collect();
        tracing::debug!(path = %persona_path.display(), chars = excerpt.chars().count(), "loaded dev persona");
        return format!(
            "{excerpt}\n\n当前角色：开发者。请用中文简洁、准确地回答，优先给出可执行方案。"
        );
    }

    if let Some(custom) = load_custom(role, data_root) {
        return custom.system_prompt;
    }

    match role {
        "custom-role" => {
            "你是小明同学的垂直领域专家角色。请用中文简洁回答，并在信息不足时说明假设。".to_string()
        }
        _ => "你是小明同学，一个本地优先的个人 AI 助手。请用中文简洁、准确地回答。".to_string(),
    }
}

fn load_custom(role: &str, data_root: &Path) -> Option<RoleSummary> {
    if !is_safe_id(role) || is_valid(role) {
        return None;
    }
    let path = roles_dir(data_root).join(format!("{role}.json"));
    let text = std::fs::read_to_string(path).ok()?;
    let mut role = serde_json::from_str::<RoleSummary>(&text).ok()?;
    role.builtin = false;
    Some(role)
}

pub fn roles_dir(data_root: &Path) -> PathBuf {
    data_root.join("roles")
}

fn is_safe_id(value: &str) -> bool {
    let length = value.chars().count();
    (1..=64).contains(&length)
        && value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
}

fn now_ms() -> u128 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|value| value.as_millis())
        .unwrap_or_default()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ai-assistant-roles-{name}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn lists_builtin_roles() {
        let root = temp_root("builtin");
        let roles = list(&root);
        assert!(roles.iter().any(|role| role.id == "dev" && role.builtin));
        assert!(roles
            .iter()
            .any(|role| role.id == "general" && role.builtin));
    }

    #[test]
    fn saves_and_loads_custom_role() {
        let root = temp_root("custom");
        let role = RoleSummary {
            id: "writer".to_string(),
            name: "写作助手".to_string(),
            desc: "写作".to_string(),
            system_prompt: "你是写作助手。".to_string(),
            model_preference: "heavy".to_string(),
            builtin: false,
            enabled: true,
        };
        save_custom(&root, role).unwrap();

        let roles = list(&root);
        assert!(roles.iter().any(|item| item.id == "writer"));
        assert!(is_valid_at("writer", &root));
        assert_eq!(system_prompt("writer", &root, &root), "你是写作助手。");
    }

    #[test]
    fn rejects_invalid_and_builtin_ids() {
        let root = temp_root("invalid");
        let role = RoleSummary {
            id: "Bad ID".to_string(),
            name: "x".to_string(),
            desc: String::new(),
            system_prompt: "x".to_string(),
            model_preference: "heavy".to_string(),
            builtin: false,
            enabled: true,
        };
        assert!(save_custom(&root, role).is_err());

        let builtin = RoleSummary {
            id: "dev".to_string(),
            name: "x".to_string(),
            desc: String::new(),
            system_prompt: "x".to_string(),
            model_preference: "heavy".to_string(),
            builtin: true,
            enabled: true,
        };
        assert!(save_custom(&root, builtin).is_err());
    }

    #[test]
    fn archives_role_data_on_delete() {
        let root = temp_root("delete");
        let role = RoleSummary {
            id: "writer".to_string(),
            name: "写作助手".to_string(),
            desc: String::new(),
            system_prompt: "你是写作助手。".to_string(),
            model_preference: "heavy".to_string(),
            builtin: false,
            enabled: true,
        };
        save_custom(&root, role).unwrap();
        let memory = root.join("memory").join("writer");
        std::fs::create_dir_all(&memory).unwrap();
        std::fs::write(memory.join("note.txt"), "keep").unwrap();

        delete_custom(&root, "writer", false).unwrap();
        assert!(!is_valid_at("writer", &root));
        assert!(!memory.exists());
        assert!(root.join("memory").join("_deleted").exists());
    }
}
