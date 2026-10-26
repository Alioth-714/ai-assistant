use crate::error::Result;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct SecretsFile {
    #[serde(default)]
    pub providers: BTreeMap<String, String>,
}

pub fn secrets_path(data_root: &Path) -> PathBuf {
    data_root.join("secrets.json")
}

pub fn load(data_root: &Path) -> SecretsFile {
    let path = secrets_path(data_root);
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| match serde_json::from_str::<SecretsFile>(&text) {
            Ok(value) => Some(value),
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "secrets parse failed");
                None
            }
        })
        .unwrap_or_default()
}

pub fn get(data_root: &Path, provider_id: &str) -> Option<String> {
    load(data_root)
        .providers
        .get(provider_id)
        .filter(|value| !value.trim().is_empty())
        .cloned()
}

pub fn has(data_root: &Path, provider_id: &str) -> bool {
    get(data_root, provider_id).is_some()
}

pub fn set(data_root: &Path, provider_id: &str, api_key: Option<&str>) -> Result<()> {
    let mut file = load(data_root);
    match api_key.map(str::trim) {
        Some(value) if !value.is_empty() => {
            file.providers
                .insert(provider_id.to_string(), value.to_string());
        }
        Some(_) => {
            file.providers.remove(provider_id);
        }
        None => {}
    }
    save(data_root, &file)
}

pub fn remove(data_root: &Path, provider_id: &str) -> Result<()> {
    let mut file = load(data_root);
    file.providers.remove(provider_id);
    save(data_root, &file)
}

fn save(data_root: &Path, file: &SecretsFile) -> Result<()> {
    let path = secrets_path(data_root);
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let text = serde_json::to_string_pretty(file)?;
    let temp = path.with_extension("json.tmp");
    std::fs::write(&temp, text)?;
    if path.exists() {
        std::fs::remove_file(&path)?;
    }
    std::fs::rename(&temp, &path)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ai-assistant-secrets-{name}-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_millis())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn stores_and_removes_provider_key() {
        let root = temp_root("roundtrip");
        set(&root, "deepseek", Some("sk-test")).unwrap();
        assert!(has(&root, "deepseek"));
        assert_eq!(get(&root, "deepseek").as_deref(), Some("sk-test"));

        set(&root, "deepseek", Some("")).unwrap();
        assert!(!has(&root, "deepseek"));
    }

    #[test]
    fn keeps_key_when_none_is_passed() {
        let root = temp_root("keep");
        set(&root, "relay", Some("sk-relay")).unwrap();
        set(&root, "relay", None).unwrap();
        assert_eq!(get(&root, "relay").as_deref(), Some("sk-relay"));
    }
}
