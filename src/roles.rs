//! 角色体系（2026-10-26 重写）
//!
//! 规则（用户已确认）：
//! - **8 个固定槽位**：内置 4 个（dev / general / health-coach / mind-companion）+ 自建 4 个（custom-5 ~ custom-8）
//! - 自建 ID 由系统分配，取最小空号；删除后立即回收；同一 ID 不会同时存在两个角色
//! - 上限 8 **包含被禁用的角色**（禁用不释放名额）
//! - 内置角色不可删、不可改 ID
//! - 删除自建角色时**彻底删除**其角色文件、记忆、知识库与上下文缓存

use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

pub const DEFAULT_ROLE: &str = "dev";

/// 内置角色 ID（顺序固定，对应 1-4 号位）
pub const BUILTIN_IDS: [&str; 4] = ["dev", "general", "health-coach", "mind-companion"];

/// 自建角色号段（对应 5-8 号位）
pub const CUSTOM_ID_MIN: u8 = 5;
pub const CUSTOM_ID_MAX: u8 = 8;

/// 自建角色上限
pub const MAX_CUSTOM_ROLES: usize = (CUSTOM_ID_MAX - CUSTOM_ID_MIN + 1) as usize;
/// 角色总数上限
pub const MAX_ROLES: usize = BUILTIN_IDS.len() + MAX_CUSTOM_ROLES;

/// 云端策略：既支持单一字符串，也支持按 profile 区分配置
/// - `"localOnly"` / `"localFirst"` / `"default"`
/// - `{"small": "localFirst", "large": "localOnly"}`
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(untagged)]
pub enum CloudPolicy {
    Named(String),
    ByProfile(BTreeMap<String, String>),
}

impl Default for CloudPolicy {
    fn default() -> Self {
        CloudPolicy::Named("default".to_string())
    }
}

impl CloudPolicy {
    /// 按当前 profile 解析出实际策略；该 profile 未配置时回退 small
    pub fn resolve(&self, profile: &str) -> String {
        match self {
            CloudPolicy::Named(value) => {
                let value = value.trim();
                if value.is_empty() {
                    "default".to_string()
                } else {
                    value.to_string()
                }
            }
            CloudPolicy::ByProfile(map) => map
                .get(profile)
                .or_else(|| map.get("small"))
                .map(|value| value.trim().to_string())
                .filter(|value| !value.is_empty())
                .unwrap_or_else(|| "default".to_string()),
        }
    }
}

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
    /// 常驻免责声明（健康 / 心理角色必填）
    #[serde(default)]
    pub disclaimer: String,
    /// full / explicitOnly（健康 / 心理建议 explicitOnly：不默认写入长期记忆）
    #[serde(default)]
    pub memory_scope: String,
    /// default / localOnly / localFirst，或按 profile 的映射
    #[serde(default)]
    pub cloud_policy: CloudPolicy,
    /// 机器可读的安全护栏（也会拼进提示词）
    #[serde(default)]
    pub guardrails: Vec<String>,
    /// concise / structured
    #[serde(default)]
    pub output_style: String,
    /// 按当前 profile 解析后的策略（只用于输出，不落盘）
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub cloud_policy_resolved: Option<String>,
}

/// 「按角色名生成提示词」的返回结构
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct GeneratedRolePrompt {
    pub system_prompt: String,
    #[serde(default)]
    pub guardrails: Vec<String>,
    #[serde(default)]
    pub output_style: String,
}

fn default_model_preference() -> String {
    "heavy".to_string()
}

fn default_true() -> bool {
    true
}

/// 内置角色定义文件位置：`config/roles/{id}.json`
pub fn builtin_role_path(app_root: &Path, id: &str) -> PathBuf {
    app_root
        .join("config")
        .join("roles")
        .join(format!("{id}.json"))
}

/// 自建角色定义文件位置：`data/roles/{id}.json`
pub fn custom_role_path(data_root: &Path, id: &str) -> PathBuf {
    roles_dir(data_root).join(format!("{id}.json"))
}

pub fn roles_dir(data_root: &Path) -> PathBuf {
    data_root.join("roles")
}

pub fn is_builtin(id: &str) -> bool {
    BUILTIN_IDS.contains(&id)
}

/// 自建 ID 是否合法（custom-5 ~ custom-8）
pub fn is_custom_id(id: &str) -> bool {
    parse_custom_number(id).is_some()
}

fn parse_custom_number(id: &str) -> Option<u8> {
    let rest = id.strip_prefix("custom-")?;
    let number: u8 = rest.parse().ok()?;
    (CUSTOM_ID_MIN..=CUSTOM_ID_MAX)
        .contains(&number)
        .then_some(number)
}

/// 加载内置角色：优先读 `config/roles/{id}.json`，缺失时用代码内置兜底
pub fn builtin_roles(app_root: &Path) -> Vec<RoleSummary> {
    BUILTIN_IDS
        .iter()
        .map(|id| load_builtin(app_root, id))
        .collect()
}

fn load_builtin(app_root: &Path, id: &str) -> RoleSummary {
    let path = builtin_role_path(app_root, id);
    match std::fs::read_to_string(&path) {
        Ok(text) => match serde_json::from_str::<RoleSummary>(&text) {
            Ok(mut role) => {
                role.id = id.to_string();
                role.builtin = true;
                role
            }
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "内置角色文件解析失败，使用兜底定义");
                fallback_builtin(id)
            }
        },
        Err(_) => fallback_builtin(id),
    }
}

fn fallback_builtin(id: &str) -> RoleSummary {
    let (name, desc) = match id {
        "dev" => ("代码助手", "搞定全栈开发、Bug 调试与架构设计"),
        "general" => ("日常助手", "轻松应对聊天问答、文案写作与信息整理"),
        "health-coach" => ("健康管家", "症状自查 · 病因推断 · 缓解建议 · 挂号科室推荐"),
        "mind-companion" => ("心灵伙伴", "情绪分析 · 心理疏导 · 健康心理构建 · 压力管理"),
        _ => ("助手", ""),
    };
    RoleSummary {
        id: id.to_string(),
        name: name.to_string(),
        desc: desc.to_string(),
        system_prompt: "你是小明同学，一个本地优先的个人 AI 助手。请用中文简洁、准确地回答。"
            .to_string(),
        model_preference: default_model_preference(),
        builtin: true,
        enabled: true,
        disclaimer: String::new(),
        memory_scope: "full".to_string(),
        cloud_policy: CloudPolicy::default(),
        guardrails: Vec::new(),
        output_style: "concise".to_string(),
        cloud_policy_resolved: None,
    }
}

/// 列出全部角色：内置在前（固定顺序），自建按序号在后
pub fn list(app_root: &Path, data_root: &Path) -> Vec<RoleSummary> {
    let mut roles = builtin_roles(app_root);
    let mut customs: Vec<RoleSummary> = Vec::new();
    if let Ok(entries) = std::fs::read_dir(roles_dir(data_root)) {
        for entry in entries.flatten() {
            let path = entry.path();
            if path.extension().and_then(|value| value.to_str()) != Some("json") {
                continue;
            }
            let Some(id) = path.file_stem().and_then(|value| value.to_str()) else {
                continue;
            };
            if !is_custom_id(id) {
                continue;
            }
            let Ok(text) = std::fs::read_to_string(&path) else {
                continue;
            };
            let Ok(mut role) = serde_json::from_str::<RoleSummary>(&text) else {
                continue;
            };
            role.id = id.to_string();
            role.builtin = false;
            customs.push(role);
        }
    }
    customs.sort_by_key(|role| parse_custom_number(&role.id).unwrap_or(u8::MAX));
    roles.extend(customs);
    roles.truncate(MAX_ROLES);
    roles
}

/// 已占用的自建序号
fn used_custom_numbers(data_root: &Path) -> Vec<u8> {
    let mut numbers = Vec::new();
    if let Ok(entries) = std::fs::read_dir(roles_dir(data_root)) {
        for entry in entries.flatten() {
            let id = entry
                .path()
                .file_stem()
                .and_then(|value| value.to_str())
                .unwrap_or_default()
                .to_string();
            if let Some(number) = parse_custom_number(&id) {
                numbers.push(number);
            }
        }
    }
    numbers
}

pub fn custom_count(data_root: &Path) -> usize {
    used_custom_numbers(data_root).len()
}

/// 分配最小空号；满额返回 None
pub fn next_custom_id(data_root: &Path) -> Option<String> {
    let used = used_custom_numbers(data_root);
    (CUSTOM_ID_MIN..=CUSTOM_ID_MAX)
        .find(|number| !used.contains(number))
        .map(|number| format!("custom-{number}"))
}

/// 角色是否存在（内置恒存在；自建需文件存在）
pub fn is_valid_at(id: &str, data_root: &Path) -> bool {
    is_builtin(id) || custom_role_path(data_root, id).is_file()
}

/// 角色定义是否存在（供运行时校验）
pub fn load_role(app_root: &Path, data_root: &Path, id: &str) -> Option<RoleSummary> {
    list(app_root, data_root)
        .into_iter()
        .find(|role| role.id == id)
}

/// 角色系统提示词：正文 + 安全护栏 + 免责声明
pub fn system_prompt(role: &str, app_root: &Path, data_root: &Path) -> String {
    let Some(role) = load_role(app_root, data_root, role) else {
        return "你是小明同学，一个本地优先的个人 AI 助手。请用中文简洁、准确地回答。".to_string();
    };

    let mut text = role.system_prompt.trim().to_string();
    if text.is_empty() {
        text = format!(
            "你是「{}」：{}。请用中文简洁、准确地回答。",
            role.name, role.desc
        );
    }
    if !role.guardrails.is_empty() {
        text.push_str("\n\n【必须遵守的安全护栏】\n");
        for item in &role.guardrails {
            text.push_str(&format!("- {item}\n"));
        }
    }
    if role.output_style == "structured" {
        text.push_str("\n【输出要求】优先使用要点/表格等结构化形式，先给结论。\n");
    }
    if !role.disclaimer.trim().is_empty() {
        text.push_str(&format!(
            "\n【免责声明（每次回答末尾需保留）】{}\n",
            role.disclaimer.trim()
        ));
    }
    text
}

/// 保存角色：`id` 为空表示新建（系统分配号），否则表示更新
pub fn save_custom(data_root: &Path, role: RoleSummary) -> Result<RoleSummary> {
    let id = role.id.trim();
    if id.is_empty() {
        create_custom(data_root, &role.name, &role.desc, &role.system_prompt)
    } else {
        update_custom(data_root, id, &role.name, &role.desc, &role.system_prompt)
    }
}

pub fn create_custom(
    data_root: &Path,
    name: &str,
    desc: &str,
    system_prompt: &str,
) -> Result<RoleSummary> {
    let name = name.trim();
    let system_prompt = system_prompt.trim();
    if name.is_empty() {
        return Err(AppError::InvalidInput("角色名称不能为空".to_string()));
    }
    if system_prompt.is_empty() {
        return Err(AppError::InvalidInput("角色系统提示词不能为空".to_string()));
    }
    let Some(id) = next_custom_id(data_root) else {
        return Err(AppError::InvalidInput(format!(
            "自建角色已达上限（{MAX_CUSTOM_ROLES}/{MAX_CUSTOM_ROLES}）"
        )));
    };

    let role = RoleSummary {
        id: id.clone(),
        name: name.to_string(),
        desc: desc.trim().to_string(),
        system_prompt: system_prompt.to_string(),
        model_preference: default_model_preference(),
        builtin: false,
        enabled: true,
        disclaimer: String::new(),
        memory_scope: "full".to_string(),
        cloud_policy: CloudPolicy::default(),
        guardrails: Vec::new(),
        output_style: "concise".to_string(),
        cloud_policy_resolved: None,
    };
    write_role(data_root, &role)?;
    tracing::info!(role = %id, "created custom role");
    Ok(role)
}

pub fn update_custom(
    data_root: &Path,
    id: &str,
    name: &str,
    desc: &str,
    system_prompt: &str,
) -> Result<RoleSummary> {
    let id = id.trim();
    if is_builtin(id) {
        return Err(AppError::InvalidInput("内置角色不可修改".to_string()));
    }
    if !is_custom_id(id) {
        return Err(AppError::InvalidInput("角色 ID 非法".to_string()));
    }
    let Some(mut existing) = read_role(data_root, id) else {
        return Err(AppError::InvalidInput("角色不存在".to_string()));
    };
    let name = name.trim();
    let system_prompt = system_prompt.trim();
    if name.is_empty() {
        return Err(AppError::InvalidInput("角色名称不能为空".to_string()));
    }
    if system_prompt.is_empty() {
        return Err(AppError::InvalidInput("角色系统提示词不能为空".to_string()));
    }

    existing.name = name.to_string();
    existing.desc = desc.trim().to_string();
    existing.system_prompt = system_prompt.to_string();
    existing.builtin = false;
    write_role(data_root, &existing)?;
    tracing::info!(role = %id, "updated custom role");
    Ok(existing)
}

/// 彻底删除自建角色：角色文件、记忆、知识库、上下文缓存
pub fn delete_custom(data_root: &Path, id: &str) -> Result<()> {
    let id = id.trim();
    if is_builtin(id) {
        return Err(AppError::InvalidInput("内置角色不可删除".to_string()));
    }
    if !is_custom_id(id) {
        return Err(AppError::InvalidInput("角色 ID 非法".to_string()));
    }

    let role_path = custom_role_path(data_root, id);
    if role_path.exists() {
        std::fs::remove_file(&role_path)?;
    }
    for dir in [
        data_root.join("memory").join(id),
        data_root.join("knowledge").join(id),
    ] {
        if dir.exists() {
            std::fs::remove_dir_all(&dir)?;
        }
    }
    let context_cache = data_root
        .join("cache")
        .join("role_contexts")
        .join(format!("{id}.json"));
    if context_cache.exists() {
        std::fs::remove_file(&context_cache)?;
    }
    tracing::info!(role = %id, "deleted custom role (data removed)");
    Ok(())
}

fn read_role(data_root: &Path, id: &str) -> Option<RoleSummary> {
    let text = std::fs::read_to_string(custom_role_path(data_root, id)).ok()?;
    let mut role: RoleSummary = serde_json::from_str(&text).ok()?;
    role.id = id.to_string();
    role.builtin = false;
    Some(role)
}

fn write_role(data_root: &Path, role: &RoleSummary) -> Result<()> {
    let dir = roles_dir(data_root);
    std::fs::create_dir_all(&dir)?;
    let path = custom_role_path(data_root, &role.id);
    let text = serde_json::to_string_pretty(role)?;
    std::fs::write(&path, text)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_data(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ai-assistant-roles-{tag}-{}-{}",
            std::process::id(),
            crate::util::now_ms()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn fake_app_root(tag: &str) -> PathBuf {
        let dir = temp_data(tag);
        std::fs::create_dir_all(dir.join("config").join("roles")).expect("mkdir config");
        dir
    }

    #[test]
    fn allocates_smallest_free_custom_id() {
        let data = temp_data("alloc");
        assert_eq!(next_custom_id(&data).as_deref(), Some("custom-5"));
        create_custom(&data, "A", "", "提示词").expect("create A");
        assert_eq!(next_custom_id(&data).as_deref(), Some("custom-6"));
        let b = create_custom(&data, "B", "", "提示词").expect("create B");
        assert_eq!(b.id, "custom-6");
        delete_custom(&data, "custom-5").expect("delete");
        assert_eq!(
            next_custom_id(&data).as_deref(),
            Some("custom-5"),
            "删除后应复用最小空号"
        );
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn refuses_when_custom_slots_full() {
        let data = temp_data("full");
        for name in ["A", "B", "C", "D"] {
            create_custom(&data, name, "", "提示词").expect("create");
        }
        assert_eq!(custom_count(&data), MAX_CUSTOM_ROLES);
        let err = create_custom(&data, "E", "", "提示词").expect_err("应拒绝");
        assert!(err.to_string().contains("上限"));
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn custom_ids_stay_inside_range() {
        assert!(is_custom_id("custom-5"));
        assert!(is_custom_id("custom-8"));
        assert!(!is_custom_id("custom-4"));
        assert!(!is_custom_id("custom-9"));
        assert!(!is_custom_id("dev"));
    }

    #[test]
    fn builtin_roles_are_protected() {
        let data = temp_data("builtin");
        assert!(delete_custom(&data, "dev").is_err());
        let err = update_custom(&data, "general", "x", "", "y").expect_err("不可改");
        assert!(err.to_string().contains("内置"));
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn delete_removes_role_memory_knowledge_and_cache() {
        let data = temp_data("cleanup");
        let role = create_custom(&data, "清理测试", "", "提示词").expect("create");
        let id = role.id.clone();
        for dir in ["memory", "knowledge"] {
            std::fs::create_dir_all(data.join(dir).join(&id)).expect("mkdir");
            std::fs::write(data.join(dir).join(&id).join("x.json"), b"{}").expect("write");
        }
        std::fs::create_dir_all(data.join("cache").join("role_contexts")).expect("mkdir");
        std::fs::write(
            data.join("cache")
                .join("role_contexts")
                .join(format!("{id}.json")),
            b"{}",
        )
        .expect("write");

        delete_custom(&data, &id).expect("delete");
        assert!(!custom_role_path(&data, &id).exists());
        assert!(!data.join("memory").join(&id).exists());
        assert!(!data.join("knowledge").join(&id).exists());
        assert!(!data
            .join("cache")
            .join("role_contexts")
            .join(format!("{id}.json"))
            .exists());
        let _ = std::fs::remove_dir_all(&data);
    }

    #[test]
    fn loads_builtin_roles_from_config_files() {
        let app = fake_app_root("builtin-load");
        std::fs::write(
            builtin_role_path(&app, "general"),
            r#"{"id":"general","name":"日常助手","desc":"测试","systemPrompt":"你是日常助手。","guardrails":["不编造事实"],"disclaimer":"以上仅供参考"}"#,
        )
        .expect("write role file");

        let roles = builtin_roles(&app);
        assert_eq!(roles.len(), BUILTIN_IDS.len());
        let general = roles
            .iter()
            .find(|role| role.id == "general")
            .expect("general");
        assert_eq!(general.name, "日常助手");
        assert!(general.builtin);

        let prompt = system_prompt("general", &app, &app.join("data"));
        assert!(prompt.contains("你是日常助手"));
        assert!(prompt.contains("不编造事实"));
        assert!(prompt.contains("以上仅供参考"));
        let _ = std::fs::remove_dir_all(&app);
    }

    #[test]
    fn falls_back_when_builtin_file_missing() {
        let app = fake_app_root("fallback");
        let roles = builtin_roles(&app);
        let dev = roles.iter().find(|role| role.id == "dev").expect("dev");
        assert_eq!(dev.name, "代码助手");
        let _ = std::fs::remove_dir_all(&app);
    }

    #[test]
    fn repo_builtin_role_files_parse() {
        // 回归防护：内置角色文件写错时 load_builtin 会静默回退，这里直接严格解析
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        for id in BUILTIN_IDS {
            let path = builtin_role_path(app_root, id);
            let text = std::fs::read_to_string(&path)
                .unwrap_or_else(|_| panic!("缺少内置角色文件：{}", path.display()));
            let role: RoleSummary = serde_json::from_str(&text)
                .unwrap_or_else(|err| panic!("{} 解析失败：{err}", path.display()));
            assert_eq!(role.id, id, "{} 的 id 字段不匹配", path.display());
            assert!(!role.name.trim().is_empty(), "{} 缺少名称", path.display());
            assert!(
                !role.system_prompt.trim().is_empty(),
                "{} 缺少提示词",
                path.display()
            );
        }
    }

    #[test]
    fn sensitive_roles_are_local_first_then_tightened_by_profile() {
        let app_root = Path::new(env!("CARGO_MANIFEST_DIR"));
        for id in ["health-coach", "mind-companion"] {
            let role = load_builtin(app_root, id);
            // small（3B+7B）：本地优先，可显式放权上云
            assert_eq!(role.cloud_policy.resolve("small"), "localFirst", "{id}");
            // large（14B+）：收紧为仅本地
            assert_eq!(role.cloud_policy.resolve("large"), "localOnly", "{id}");
            assert_eq!(role.memory_scope, "explicitOnly", "{id} 不应默认写记忆");
            assert!(!role.disclaimer.trim().is_empty(), "{id} 需要免责声明");
            assert!(!role.guardrails.is_empty(), "{id} 需要安全护栏");
        }
    }

    #[test]
    fn list_puts_builtin_first_and_custom_by_number() {
        let app = fake_app_root("order");
        let data = temp_data("order-data");
        create_custom(&data, "甲", "", "提示词").expect("create");
        create_custom(&data, "乙", "", "提示词").expect("create");
        let ids: Vec<String> = list(&app, &data).into_iter().map(|role| role.id).collect();
        assert_eq!(
            ids,
            vec![
                "dev",
                "general",
                "health-coach",
                "mind-companion",
                "custom-5",
                "custom-6"
            ]
        );
        let _ = std::fs::remove_dir_all(&app);
        let _ = std::fs::remove_dir_all(&data);
    }
}
