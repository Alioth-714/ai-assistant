use crate::config::{AppConfig, ModelsConfig};
use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelSetting {
    /// 目录内唯一 ID：`{providerId}:{modelId}`
    pub id: String,
    pub provider_id: String,
    pub model_id: String,
    #[serde(default)]
    pub display_name: String,
    /// 是否用真实请求验证过
    #[serde(default)]
    pub verified: bool,
    /// 是否启用（决定是否出现在聊天页的模型列表）
    #[serde(default = "default_true")]
    pub enabled: bool,
    /// 排序序号（越小越靠前，供应商内独立）
    #[serde(default)]
    pub sort_index: i64,
    /// 上次失败原因（无则为空）
    #[serde(default)]
    pub last_error: String,
}

fn default_true() -> bool {
    true
}

impl ModelSetting {
    pub fn make_id(provider_id: &str, model_id: &str) -> String {
        format!("{provider_id}:{model_id}")
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UserSettings {
    pub theme: String,
    pub models: ModelSettings,
    pub data: DataSettings,
    /// 外部模型目录（供应商层 → 模型层）
    #[serde(default)]
    pub external_models: Vec<ModelSetting>,
    pub onboarding_completed: bool,
    pub providers: Vec<ProviderSettings>,
    pub default_chat: String,
    pub cloud: CloudSettings,
    pub file_export_grants: Vec<FileExportGrant>,
    /// 本机实测成功过的供应商 id（实测过一次即视为已验证）
    #[serde(default)]
    pub verified_providers: Vec<String>,
    /// 已应用的供应商预设版本（用于提示"预设有更新"）
    #[serde(default)]
    pub preset_version: u32,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            theme: "system".to_string(),
            models: ModelSettings::default(),
            data: DataSettings::default(),
            external_models: Vec::new(),
            onboarding_completed: false,
            providers: Vec::new(),
            default_chat: "local".to_string(),
            cloud: CloudSettings::default(),
            file_export_grants: Vec::new(),
            verified_providers: Vec::new(),
            preset_version: 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ModelSettings {
    pub ollama_host: String,
    pub router: String,
    pub coordinator: String,
    pub models_dir: String,
}

impl Default for ModelSettings {
    fn default() -> Self {
        Self {
            ollama_host: "http://127.0.0.1:11434".to_string(),
            router: String::new(),
            coordinator: String::new(),
            models_dir: String::new(),
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DataSettings {
    pub root: String,
    pub logs_dir: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct ProviderSettings {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    pub models: Vec<String>,
    pub default_model: String,
    /// 协议：openai-chat（默认）/ openai-responses / anthropic-messages
    #[serde(default)]
    pub protocol: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct CloudSettings {
    pub enabled: bool,
    pub mode: String,
    /// 「仅本地」开关的反面（决策 33）：false=仅本地（默认），true=自动模式，复杂任务可自动上云。
    pub auto_escalate: bool,
    /// 能力档位 profile（small=3B+7B，large=14B+）
    pub profile: String,
}

impl Default for CloudSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: "localFirst".to_string(),
            auto_escalate: false,
            profile: "small".to_string(),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileExportGrant {
    pub role_id: String,
    pub provider_id: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SettingsView {
    pub settings: UserSettings,
    pub data_root: String,
    pub logs_dir: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct SaveSettingsResult {
    pub view: SettingsView,
    pub restart_required: bool,
}

pub fn settings_path(data_root: &Path) -> PathBuf {
    data_root.join("settings.json")
}

pub fn load(data_root: &Path, app: &AppConfig, models: &ModelsConfig) -> UserSettings {
    let path = settings_path(data_root);
    let mut settings = std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| match serde_json::from_str::<UserSettings>(&text) {
            Ok(value) => Some(value),
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "settings parse failed, using defaults");
                None
            }
        })
        .unwrap_or_default();

    if settings.models.ollama_host.trim().is_empty() {
        settings.models.ollama_host = "http://127.0.0.1:11434".to_string();
    }
    if settings.models.router.trim().is_empty() {
        settings.models.router = app.router.model.clone();
    }
    if settings.models.coordinator.trim().is_empty() {
        settings.models.coordinator = app.coordinator.model.clone();
    }
    if settings.models.models_dir.trim().is_empty() {
        settings.models.models_dir = models.models_dir.clone();
    }
    if !matches!(settings.theme.as_str(), "system" | "light" | "dark") {
        settings.theme = "system".to_string();
    }
    if !matches!(settings.cloud.mode.as_str(), "localFirst" | "cloudOnly") {
        settings.cloud.mode = "localFirst".to_string();
    }
    if settings.default_chat.trim().is_empty() {
        settings.default_chat = "local".to_string();
    }
    migrate_external_models(&mut settings);
    settings
}

/// 旧配置迁移：把 `providers[].defaultModel` 提升为独立的模型目录，并把 defaultChat 指向模型
pub fn migrate_external_models(settings: &mut UserSettings) {
    for provider in &settings.providers {
        let mut candidates: Vec<String> = provider
            .models
            .iter()
            .map(|item| item.trim().to_string())
            .filter(|item| !item.is_empty())
            .collect();
        if !provider.default_model.trim().is_empty() {
            candidates.push(provider.default_model.trim().to_string());
        }
        for model_id in candidates {
            let id = ModelSetting::make_id(&provider.id, &model_id);
            if settings.external_models.iter().any(|item| item.id == id) {
                continue;
            }
            settings.external_models.push(ModelSetting {
                id,
                provider_id: provider.id.clone(),
                model_id,
                display_name: String::new(),
                verified: true,
                enabled: true,
                sort_index: 0,
                last_error: String::new(),
            });
        }
    }

    // 旧的 defaultChat 存的是供应商 id，改成该供应商的默认模型 id
    if let Some(provider) = settings
        .providers
        .iter()
        .find(|item| item.id == settings.default_chat)
    {
        if !provider.default_model.trim().is_empty() {
            settings.default_chat =
                ModelSetting::make_id(&provider.id, provider.default_model.trim());
        }
    }
    // 指到不存在的模型就退回本地
    if settings.default_chat != "local"
        && !settings
            .external_models
            .iter()
            .any(|item| item.id == settings.default_chat)
    {
        settings.default_chat = "local".to_string();
    }
}

/// 规范化模型目录：去空、去重、重建 id、剔除已不存在的供应商，并校正 defaultChat
pub fn normalize_external_models(settings: &mut UserSettings) {
    let providers: Vec<String> = settings
        .providers
        .iter()
        .map(|item| item.id.clone())
        .collect();
    let mut cleaned: Vec<ModelSetting> = Vec::new();
    for entry in settings.external_models.drain(..) {
        let provider_id = entry.provider_id.trim().to_string();
        let model_id = entry.model_id.trim().to_string();
        if provider_id.is_empty() || model_id.is_empty() {
            continue;
        }
        if !providers.iter().any(|id| id == &provider_id) {
            continue;
        }
        let id = ModelSetting::make_id(&provider_id, &model_id);
        if cleaned.iter().any(|item| item.id == id) {
            continue;
        }
        cleaned.push(ModelSetting {
            id,
            provider_id,
            model_id,
            display_name: entry.display_name.trim().to_string(),
            verified: entry.verified,
            enabled: entry.enabled,
            sort_index: entry.sort_index,
            last_error: entry.last_error.trim().to_string(),
        });
    }
    settings.external_models = cleaned;

    if settings.default_chat != "local"
        && !settings
            .external_models
            .iter()
            .any(|item| item.id == settings.default_chat)
    {
        settings.default_chat = "local".to_string();
    }
}

pub fn save(data_root: &Path, mut settings: UserSettings) -> Result<UserSettings> {
    migrate_external_models(&mut settings);
    normalize_external_models(&mut settings);
    if !matches!(settings.theme.as_str(), "system" | "light" | "dark") {
        settings.theme = "system".to_string();
    }
    if !matches!(settings.cloud.mode.as_str(), "localFirst" | "cloudOnly") {
        settings.cloud.mode = "localFirst".to_string();
    }
    settings.default_chat = settings.default_chat.trim().to_string();
    if settings.default_chat.is_empty() {
        settings.default_chat = "local".to_string();
    }
    settings.models.ollama_host = settings.models.ollama_host.trim().to_string();
    settings.models.router = settings.models.router.trim().to_string();
    settings.models.coordinator = settings.models.coordinator.trim().to_string();
    settings.models.models_dir = settings.models.models_dir.trim().to_string();
    settings.data.root = settings.data.root.trim().to_string();
    settings.data.logs_dir = settings.data.logs_dir.trim().to_string();

    let path = settings_path(data_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(&settings)?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text)?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    std::fs::rename(&temp, &path)?;
    tracing::info!(path = %path.display(), "saved user settings");
    Ok(settings)
}

pub fn view(data_root: &Path, settings: UserSettings) -> SettingsView {
    SettingsView {
        settings,
        data_root: data_root.display().to_string(),
        logs_dir: data_root.join("logs").display().to_string(),
    }
}

pub fn apply_model_overrides(settings: &UserSettings, app: &mut AppConfig) {
    if !settings.models.router.trim().is_empty() {
        app.router.model = settings.models.router.clone();
    }
    if !settings.models.coordinator.trim().is_empty() {
        app.coordinator.model = settings.models.coordinator.clone();
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ai-assistant-settings-{name}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    fn fixture() -> (AppConfig, ModelsConfig) {
        let root = Path::new(env!("CARGO_MANIFEST_DIR"));
        crate::config::load(root).expect("load config")
    }

    fn now_ms() -> u128 {
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_millis())
            .unwrap_or_default()
    }

    #[test]
    fn defaults_fill_models_from_config() {
        let root = temp_root("defaults");
        let (app, models) = fixture();
        let settings = load(&root, &app, &models);
        assert_eq!(settings.models.router, app.router.model);
        assert_eq!(settings.models.coordinator, app.coordinator.model);
        assert_eq!(settings.theme, "system");
    }

    #[test]
    fn roundtrips_settings() {
        let root = temp_root("roundtrip");
        let (app, models) = fixture();
        let mut settings = load(&root, &app, &models);
        settings.theme = "dark".to_string();
        settings.models.router = "custom-router".to_string();
        settings.onboarding_completed = true;
        save(&root, settings).unwrap();

        let loaded = load(&root, &app, &models);
        assert_eq!(loaded.theme, "dark");
        assert_eq!(loaded.models.router, "custom-router");
        assert!(loaded.onboarding_completed);
    }

    #[test]
    fn normalizes_invalid_theme_and_mode() {
        let root = temp_root("normalize");
        let (app, models) = fixture();
        let mut settings = load(&root, &app, &models);
        settings.theme = "neon".to_string();
        settings.cloud.mode = "cloud".to_string();
        let saved = save(&root, settings).unwrap();
        assert_eq!(saved.theme, "system");
        assert_eq!(saved.cloud.mode, "localFirst");
    }

    #[test]
    fn applies_model_overrides() {
        let (mut app, models) = fixture();
        let root = temp_root("overrides");
        let mut settings = load(&root, &app, &models);
        settings.models.router = "router-x".to_string();
        settings.models.coordinator = "coordinator-x".to_string();
        apply_model_overrides(&settings, &mut app);
        assert_eq!(app.router.model, "router-x");
        assert_eq!(app.coordinator.model, "coordinator-x");
    }

    #[test]
    fn migrates_provider_default_model_into_catalog() {
        let mut settings = UserSettings::default();
        settings.providers.push(ProviderSettings {
            id: "deepseek".to_string(),
            name: "DeepSeek".to_string(),
            base_url: "https://api.deepseek.com/v1".to_string(),
            enabled: true,
            models: vec!["deepseek-reasoner".to_string()],
            default_model: "deepseek-chat".to_string(),
            protocol: "openai-chat".to_string(),
        });
        settings.default_chat = "deepseek".to_string();

        migrate_external_models(&mut settings);

        let ids: Vec<String> = settings
            .external_models
            .iter()
            .map(|item| item.id.clone())
            .collect();
        assert!(ids.contains(&"deepseek:deepseek-chat".to_string()));
        assert!(ids.contains(&"deepseek:deepseek-reasoner".to_string()));
        assert_eq!(settings.default_chat, "deepseek:deepseek-chat");
    }

    #[test]
    fn normalizes_external_models_and_default_chat() {
        let mut settings = UserSettings::default();
        settings.providers.push(ProviderSettings {
            id: "deepseek".to_string(),
            name: "DeepSeek".to_string(),
            base_url: "https://api.deepseek.com/v1".to_string(),
            enabled: true,
            models: vec![],
            default_model: "deepseek-chat".to_string(),
            protocol: "openai-chat".to_string(),
        });
        settings.external_models = vec![
            ModelSetting {
                id: " wrong ".to_string(),
                provider_id: " deepseek ".to_string(),
                model_id: " deepseek-chat ".to_string(),
                display_name: String::new(),
                verified: false,
                enabled: true,
                sort_index: 0,
                last_error: String::new(),
            },
            ModelSetting {
                id: "dup".to_string(),
                provider_id: "deepseek".to_string(),
                model_id: "deepseek-chat".to_string(),
                display_name: String::new(),
                verified: false,
                enabled: true,
                sort_index: 0,
                last_error: String::new(),
            },
            ModelSetting {
                id: "ghost:model".to_string(),
                provider_id: "ghost".to_string(),
                model_id: "model".to_string(),
                display_name: String::new(),
                verified: false,
                enabled: true,
                sort_index: 0,
                last_error: String::new(),
            },
        ];
        settings.default_chat = "deepseek:deepseek-chat".to_string();

        normalize_external_models(&mut settings);

        assert_eq!(settings.external_models.len(), 1, "去重并剔除未知供应商");
        assert_eq!(settings.external_models[0].id, "deepseek:deepseek-chat");
        assert_eq!(settings.default_chat, "deepseek:deepseek-chat");
    }

    #[test]
    fn resets_default_chat_when_model_missing() {
        let mut settings = UserSettings {
            default_chat: "ghost:missing".to_string(),
            ..Default::default()
        };
        migrate_external_models(&mut settings);
        assert_eq!(settings.default_chat, "local");
    }
}
