use std::path::Path;

pub const DEFAULT_ROLE: &str = "dev";
pub const ROLE_IDS: [&str; 3] = ["dev", "general", "custom-role"];

pub fn is_valid(role: &str) -> bool {
    ROLE_IDS.contains(&role)
}

pub fn display_name(role: &str) -> &'static str {
    match role {
        "dev" => "开发者",
        "general" => "通用用户",
        "custom-role" => "垂直角色",
        _ => "通用用户",
    }
}

pub fn system_prompt(role: &str, app_root: &Path) -> String {
    if role == "dev" {
        let persona_path = app_root.join("docs").join("agent.md");
        if let Ok(text) = std::fs::read_to_string(&persona_path) {
            let excerpt: String = text.chars().take(1200).collect();
            tracing::debug!(path = %persona_path.display(), chars = excerpt.chars().count(), "loaded dev persona");
            return format!(
                "{excerpt}\n\n当前角色：开发者。请用中文简洁、准确地回答，优先给出可执行方案。"
            );
        }
    }

    match role {
        "dev" => "你是小明同学，一个本地优先的个人 AI 开发助手。请用中文简洁、准确地回答，优先给出可执行方案。".to_string(),
        "custom-role" => "你是小明同学的垂直领域专家角色。请用中文简洁回答，并在信息不足时说明假设。".to_string(),
        _ => "你是小明同学，一个本地优先的个人 AI 助手。请用中文简洁、准确地回答。".to_string(),
    }
}
