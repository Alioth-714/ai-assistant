use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    pub app: AppSection,
    pub router: RouterConfig,
    pub coordinator: CoordinatorConfig,
    #[serde(default)]
    pub logging: LoggingConfig,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppSection {
    pub name: String,
    pub display_name: String,
    pub version: String,
    #[serde(default)]
    pub data_dir: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RouterConfig {
    pub model: String,
    #[serde(default = "default_max_input_tokens")]
    pub max_input_tokens: u32,
    #[serde(default = "default_max_output_tokens")]
    pub max_output_tokens: u32,
    #[serde(default = "default_router_timeout")]
    pub timeout_ms: u64,
    #[serde(default = "default_confidence_threshold")]
    pub confidence_threshold: f64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct CoordinatorConfig {
    pub model: String,
    #[serde(default = "default_coordinator_tokens")]
    pub max_tokens: u32,
    #[serde(default = "default_max_sub_agents")]
    pub max_sub_agents: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LoggingConfig {
    #[serde(default = "default_log_level")]
    pub level: String,
    #[serde(default = "default_log_file")]
    pub output_file: String,
    #[serde(default = "default_true")]
    pub output_console: bool,
    #[serde(default = "default_true")]
    pub audit_enabled: bool,
}

impl Default for LoggingConfig {
    fn default() -> Self {
        Self {
            level: default_log_level(),
            output_file: default_log_file(),
            output_console: true,
            audit_enabled: true,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelsConfig {
    #[serde(default)]
    pub models_dir: String,
    #[serde(default)]
    pub grades: Grades,
    #[serde(default)]
    pub installed: BTreeMap<String, InstalledModel>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Grades {
    pub light: String,
    pub heavy: String,
    pub dev: String,
    #[serde(default)]
    pub cloud: Option<String>,
}

impl Default for Grades {
    fn default() -> Self {
        Self {
            light: "qwen25-3b".to_string(),
            heavy: "qwen25-7b".to_string(),
            dev: "qwen3:8b".to_string(),
            cloud: None,
        }
    }
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct InstalledModel {
    #[serde(default)]
    pub grade: String,
    #[serde(default)]
    pub quantization: String,
    #[serde(default)]
    pub size_gb: f64,
}

fn default_max_input_tokens() -> u32 {
    4096
}

fn default_max_output_tokens() -> u32 {
    256
}

fn default_router_timeout() -> u64 {
    2000
}

fn default_confidence_threshold() -> f64 {
    0.5
}

fn default_coordinator_tokens() -> u32 {
    8192
}

fn default_max_sub_agents() -> u32 {
    2
}

fn default_log_level() -> String {
    "info".to_string()
}

fn default_log_file() -> String {
    "logs/app.log".to_string()
}

fn default_true() -> bool {
    true
}

pub fn app_root() -> PathBuf {
    if let Ok(value) = std::env::var("AI_ASSISTANT_HOME") {
        if !value.trim().is_empty() {
            return PathBuf::from(value);
        }
    }

    let manifest = PathBuf::from(env!("CARGO_MANIFEST_DIR"));
    if cfg!(debug_assertions) && manifest.join("config").join("app.json").exists() {
        return manifest;
    }

    if let Ok(exe) = std::env::current_exe() {
        if let Some(dir) = exe.parent() {
            if dir.join("config").join("app.json").exists() {
                return dir.to_path_buf();
            }
        }
    }

    manifest
}

pub fn load(root: &Path) -> Result<(AppConfig, ModelsConfig)> {
    let app_path = root.join("config").join("app.json");
    let models_path = root.join("config").join("models.json");

    let app_text = std::fs::read_to_string(&app_path)
        .map_err(|err| AppError::Config(format!("读取 {} 失败：{err}", app_path.display())))?;
    let models_text = std::fs::read_to_string(&models_path)
        .map_err(|err| AppError::Config(format!("读取 {} 失败：{err}", models_path.display())))?;

    let mut app: AppConfig = serde_json::from_str(&app_text)
        .map_err(|err| AppError::Config(format!("解析 {} 失败：{err}", app_path.display())))?;
    let models: ModelsConfig = serde_json::from_str(&models_text)
        .map_err(|err| AppError::Config(format!("解析 {} 失败：{err}", models_path.display())))?;

    if app.router.model.trim().is_empty() {
        app.router.model = models.grades.light.clone();
    }
    if app.coordinator.model.trim().is_empty() {
        app.coordinator.model = models.grades.heavy.clone();
    }

    Ok((app, models))
}
