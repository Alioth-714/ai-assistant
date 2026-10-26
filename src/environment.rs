use crate::config::AppConfig;
use crate::ollama::OllamaClient;
use serde::{Deserialize, Serialize};
use std::time::Duration;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EnvironmentReport {
    pub ok: bool,
    pub ollama_reachable: bool,
    pub ollama_base_url: String,
    pub required_models: Vec<String>,
    pub installed_models: Vec<String>,
    pub missing_models: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    pub guidance: Vec<String>,
}

pub fn required_models(config: &AppConfig) -> Vec<String> {
    let mut models = Vec::new();
    for model in [
        &config.router.model,
        &config.coordinator.model,
        &config.coordinator.sub_agent_model,
    ] {
        let model = model.trim();
        if !model.is_empty() && !models.iter().any(|item| item == model) {
            models.push(model.to_string());
        }
    }
    models
}

pub fn model_available(installed: &[String], required: &str) -> bool {
    let required = normalize(required);
    installed.iter().any(|name| normalize(name) == required)
}

pub async fn check(ollama: &OllamaClient, config: &AppConfig) -> EnvironmentReport {
    let required_models = required_models(config);
    let base_url = ollama.base_url().to_string();
    let installed = ollama.list_models(Duration::from_secs(2)).await;

    match installed {
        Ok(installed_models) => {
            let missing_models = required_models
                .iter()
                .filter(|model| !model_available(&installed_models, model))
                .cloned()
                .collect::<Vec<_>>();
            let mut guidance = Vec::new();
            for model in &missing_models {
                guidance.push(format!(
                    "缺少模型 {model}，请在终端执行：ollama pull {model}"
                ));
            }
            if !missing_models.is_empty() {
                guidance.push("拉取完成后点击“重试检测”。".to_string());
            }
            EnvironmentReport {
                ok: missing_models.is_empty(),
                ollama_reachable: true,
                ollama_base_url: base_url,
                required_models,
                installed_models,
                missing_models,
                error: None,
                guidance,
            }
        }
        Err(err) => EnvironmentReport {
            ok: false,
            ollama_reachable: false,
            ollama_base_url: base_url,
            required_models,
            installed_models: Vec::new(),
            missing_models: Vec::new(),
            error: Some(err.to_string()),
            guidance: vec![
                "未连接到 Ollama。请先安装并启动 Ollama：https://ollama.com/download".to_string(),
                "启动命令：ollama serve".to_string(),
                "启动后点击“重试检测”。".to_string(),
            ],
        },
    }
}

fn normalize(model: &str) -> &str {
    model.trim().strip_suffix(":latest").unwrap_or(model.trim())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_models_with_optional_latest_tag() {
        let installed = vec!["qwen25-3b:latest".to_string(), "qwen25-7b".to_string()];
        assert!(model_available(&installed, "qwen25-3b"));
        assert!(model_available(&installed, "qwen25-7b:latest"));
        assert!(!model_available(&installed, "not-installed-model"));
    }

    #[test]
    fn required_models_are_deduplicated() {
        let (config, _) = crate::config::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
            .expect("load config");
        let models = required_models(&config);
        assert!(models.len() >= 2);
        assert_eq!(
            models.len(),
            models
                .iter()
                .collect::<std::collections::HashSet<_>>()
                .len()
        );
    }

    #[tokio::test]
    #[ignore = "requires local Ollama with required models"]
    async fn real_environment_is_ready() {
        let (config, _) = crate::config::load(std::path::Path::new(env!("CARGO_MANIFEST_DIR")))
            .expect("load config");
        let ollama = OllamaClient::new("http://127.0.0.1:11434").expect("ollama client");
        let report = check(&ollama, &config).await;
        assert!(report.ollama_reachable, "{:?}", report.error);
        assert!(report.ok, "missing models: {:?}", report.missing_models);
    }
}
