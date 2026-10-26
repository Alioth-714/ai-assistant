use crate::config::{AppConfig, ModelsConfig};
use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct UserSettings {
    pub theme: String,
    pub models: ModelSettings,
    pub data: DataSettings,
    pub onboarding_completed: bool,
    pub providers: Vec<ProviderSettings>,
    pub default_chat: String,
    pub cloud: CloudSettings,
    pub file_export_grants: Vec<FileExportGrant>,
}

impl Default for UserSettings {
    fn default() -> Self {
        Self {
            theme: "system".to_string(),
            models: ModelSettings::default(),
            data: DataSettings::default(),
            onboarding_completed: false,
            providers: Vec::new(),
            default_chat: "local".to_string(),
            cloud: CloudSettings::default(),
            file_export_grants: Vec::new(),
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
    /// 会话云端预算上限（token）；0 表示不限。超额自动降级为仅本地。
    pub budget_tokens: u64,
    /// 能力档位 profile（small=3B+7B，large=14B+）
    pub profile: String,
}

impl Default for CloudSettings {
    fn default() -> Self {
        Self {
            enabled: false,
            mode: "localFirst".to_string(),
            auto_escalate: false,
            budget_tokens: 50_000,
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
    settings
}

pub fn save(data_root: &Path, mut settings: UserSettings) -> Result<UserSettings> {
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
}
