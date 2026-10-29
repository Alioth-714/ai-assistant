use crate::error::{AppError, Result};
use crate::settings::ProviderSettings;
use serde::{Deserialize, Serialize};
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

const HEALTH_TTL: Duration = Duration::from_secs(300);
const TEST_TIMEOUT: Duration = Duration::from_millis(1500);
const CHAT_TIMEOUT: Duration = Duration::from_secs(120);
/// 模型列表请求的超时（TLS 握手 + 响应，宽松一些避免误判为不可达）
const MODELS_TIMEOUT: Duration = Duration::from_secs(10);
const MAX_EXPORT_FILE_BYTES: usize = 64 * 1024;
const MAX_EXPORT_FILES: usize = 5;
const MAX_EXPORT_TOTAL_BYTES: usize = 128 * 1024;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderView {
    pub id: String,
    pub name: String,
    pub base_url: String,
    pub enabled: bool,
    pub models: Vec<String>,
    pub default_model: String,
    pub has_api_key: bool,
    pub reachable: Option<bool>,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderInput {
    #[serde(default)]
    pub id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub base_url: String,
    #[serde(default)]
    pub enabled: bool,
    #[serde(default)]
    pub models: Vec<String>,
    #[serde(default)]
    pub default_model: String,
    #[serde(default)]
    pub api_key: Option<String>,
    #[serde(default)]
    pub protocol: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderTest {
    pub id: String,
    pub reachable: bool,
    pub cached: bool,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExportGate {
    pub role_id: String,
    pub provider_id: String,
    pub paths: Vec<String>,
    pub granted: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TokenUsage {
    pub prompt_tokens: u64,
    pub completion_tokens: u64,
    pub total_tokens: u64,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalReply {
    pub provider_id: String,
    pub role: String,
    pub model: String,
    pub content: String,
    pub elapsed_ms: u64,
    pub usage: Option<TokenUsage>,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct ExternalChatOutcome {
    pub status: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reply: Option<ExternalReply>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub gate: Option<ExportGate>,
}

impl ExternalChatOutcome {
    pub fn ok(reply: ExternalReply) -> Self {
        Self {
            status: "ok".to_string(),
            reply: Some(reply),
            gate: None,
        }
    }

    pub fn export_required(gate: ExportGate) -> Self {
        Self {
            status: "exportRequired".to_string(),
            reply: None,
            gate: Some(gate),
        }
    }
}

#[derive(Debug, Clone)]
pub struct HealthEntry {
    pub checked_at: Instant,
    pub reachable: bool,
}

impl HealthEntry {
    pub fn fresh(&self) -> bool {
        self.checked_at.elapsed() < HEALTH_TTL
    }
}

/// 清洗供应商名称：剥掉装饰后缀并限制长度（防止 UI 文案落库）
pub fn clean_name(name: &str) -> String {
    let mut text = name.trim().to_string();
    for suffix in [" · 海外", " · 未实测", " · 已停用", " · 国内", " · 推荐"] {
        while text.ends_with(suffix) {
            text.truncate(text.len() - suffix.len());
            text = text.trim_end().to_string();
        }
    }
    if text.chars().count() > 40 {
        text = text.chars().take(40).collect();
    }
    text
}

/// 把连接错误翻译成能直接照着排查的中文诊断
pub fn classify_error(message: &str) -> String {
    let lower = message.to_lowercase();
    if message.contains("HTTP 401") || message.contains("HTTP 403") {
        "Key 无效或没有权限（HTTP 401/403）".to_string()
    } else if message.contains("HTTP 404") {
        "接口或模型不存在（HTTP 404）：请检查 Base URL 与模型名".to_string()
    } else if lower.contains("timed out") || lower.contains("timeout") {
        "连接超时：请检查网络或代理".to_string()
    } else if lower.contains("dns")
        || lower.contains("failed to lookup")
        || lower.contains("connect")
        || lower.contains("refused")
    {
        "无法连接到服务器：请检查网络、Base URL 或代理设置".to_string()
    } else if message.contains("HTTP 402") {
        "账户余额不足（HTTP 402），请充值后重试".to_string()
    } else if message.contains("HTTP 429") {
        "请求过于频繁（HTTP 429），请稍后再试".to_string()
    } else if message.contains("HTTP 5") {
        "服务端错误（HTTP 5xx），可稍后重试".to_string()
    } else {
        message.to_string()
    }
}

pub struct ExternalClient {
    http: reqwest::Client,
}

impl ExternalClient {
    pub fn new() -> Result<Self> {
        let http = reqwest::Client::builder()
            .timeout(CHAT_TIMEOUT)
            .build()
            .map_err(|err| AppError::External(format!("初始化 HTTP 客户端失败：{err}")))?;
        Ok(Self { http })
    }

    pub async fn test(&self, provider: &ProviderSettings, api_key: &str) -> Result<()> {
        let url = endpoint(&provider.base_url, "models");
        let future = self.http.get(&url).bearer_auth(api_key).send();
        let response = tokio::time::timeout(TEST_TIMEOUT, future)
            .await
            .map_err(|_| AppError::External("Provider 连接超时（1.5 秒）".to_string()))?
            .map_err(|err| AppError::External(format!("Provider 连接失败：{err}")))?;
        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::External(format!(
                "Provider 返回 HTTP {status}：{}",
                truncate(&body, 240)
            )));
        }
        Ok(())
    }

    /// 获取该 Provider 的模型列表（自动获取模型的底层实现）。
    /// 依次尝试 `{base}/models` 与 `{base}/v1/models`，兼容三种返回形态。
    pub async fn list_models(&self, base_url: &str, api_key: &str) -> Result<Vec<String>> {
        let base = base_url.trim().trim_end_matches('/');
        if !(base.starts_with("http://") || base.starts_with("https://")) {
            return Err(AppError::External(
                "Base URL 必须以 http:// 或 https:// 开头".to_string(),
            ));
        }
        let mut candidates = vec![endpoint(base, "models")];
        if !base.ends_with("/v1") {
            candidates.push(endpoint(base, "v1/models"));
        }
        // 部分网关默认分页，追加 limit 参数尝试拿全量
        candidates.push(format!("{}?limit=1000", endpoint(base, "v1/models")));
        if base.ends_with("/v1") {
            candidates.push(format!("{}?limit=1000", endpoint(base, "models")));
        }

        let mut last_error = String::from("无法获取模型列表");
        let mut collected: Vec<String> = Vec::new();
        for url in candidates {
            let mut request = self.http.get(&url);
            if !api_key.trim().is_empty() {
                request = request.bearer_auth(api_key.trim());
            }
            let started = Instant::now();
            let response = match tokio::time::timeout(MODELS_TIMEOUT, request.send()).await {
                Ok(Ok(response)) => response,
                Ok(Err(err)) => {
                    tracing::warn!(
                        url = %url,
                        elapsed_ms = started.elapsed().as_millis() as u64,
                        error = %err,
                        "models request failed (transport)"
                    );
                    last_error = format!("请求 {url} 失败：{}", error_chain(&err));
                    continue;
                }
                Err(_) => {
                    tracing::warn!(
                        url = %url,
                        timeout_ms = MODELS_TIMEOUT.as_millis() as u64,
                        "models request timed out"
                    );
                    last_error = format!("请求 {url} 超时（{}s）", MODELS_TIMEOUT.as_secs());
                    continue;
                }
            };
            let status = response.status();
            if !status.is_success() {
                last_error = format!("{url} 返回 HTTP {status}");
                continue;
            }
            let body: serde_json::Value = match response.json().await {
                Ok(value) => value,
                Err(err) => {
                    last_error = format!("{url} 返回内容无法解析：{err}");
                    continue;
                }
            };
            let models = parse_model_list(&body);
            if !models.is_empty() {
                tracing::info!(url = %url, count = models.len(), "provider models fetched");
                collected.extend(models);
            } else {
                last_error = format!("{url} 返回空模型列表");
            }
        }
        // 合并多个端点的结果：某些网关 /models 与 /v1/models 返回的清单不同，取并集
        collected.sort();
        collected.dedup();
        if collected.is_empty() {
            return Err(AppError::External(last_error));
        }
        tracing::info!(count = collected.len(), "provider models merged");
        Ok(collected)
    }

    pub async fn chat(
        &self,
        provider: &ProviderSettings,
        api_key: &str,
        model: &str,
        messages: &[(String, String)],
    ) -> Result<ExternalReply> {
        // 协议守卫：第一版只实现 OpenAI chat/completions
        if !provider.protocol.trim().is_empty() && provider.protocol != "openai-chat" {
            return Err(AppError::External(format!(
                "暂不支持 {} 协议（当前仅支持 openai-chat）；请换用走 chat/completions 的模型",
                provider.protocol
            )));
        }
        let url = endpoint(&provider.base_url, "chat/completions");
        let payload = serde_json::json!({
            "model": model,
            "messages": messages
                .iter()
                .map(|(role, content)| serde_json::json!({
                    "role": role,
                    "content": content
                }))
                .collect::<Vec<_>>(),
            "stream": false,
            "temperature": 0.3
        });

        let started = Instant::now();
        let future = self
            .http
            .post(&url)
            .bearer_auth(api_key)
            .json(&payload)
            .send();
        let response = tokio::time::timeout(CHAT_TIMEOUT, future)
            .await
            .map_err(|_| AppError::External("外部模型调用超时".to_string()))?
            .map_err(|err| AppError::External(format!("外部模型连接失败：{err}")))?;

        if !response.status().is_success() {
            let status = response.status();
            let body = response.text().await.unwrap_or_default();
            return Err(AppError::External(format!(
                "外部模型返回 HTTP {status}：{}",
                truncate(&body, 400)
            )));
        }

        let value: serde_json::Value = response
            .json()
            .await
            .map_err(|err| AppError::External(format!("解析外部模型响应失败：{err}")))?;

        let content = extract_content(&value).ok_or_else(|| {
            AppError::External("外部模型响应缺少 choices[0].message.content".to_string())
        })?;
        let model = value
            .get("model")
            .and_then(|value| value.as_str())
            .unwrap_or(model)
            .to_string();
        let usage = extract_usage(&value);

        Ok(ExternalReply {
            provider_id: provider.id.clone(),
            role: String::new(),
            model,
            content,
            elapsed_ms: started.elapsed().as_millis() as u64,
            usage,
        })
    }
}

pub fn normalize(input: ProviderInput) -> Result<ProviderSettings> {
    let id = input.id.trim().to_string();
    let name = input.name.trim().to_string();
    let base_url = input.base_url.trim().trim_end_matches('/').to_string();
    let mut models = input
        .models
        .into_iter()
        .map(|model| model.trim().to_string())
        .filter(|model| !model.is_empty())
        .collect::<Vec<_>>();
    models.sort();
    models.dedup();

    let mut default_model = input.default_model.trim().to_string();
    if default_model.is_empty() {
        default_model = models.first().cloned().unwrap_or_default();
    }
    if default_model.is_empty() {
        return Err(AppError::InvalidInput(
            "Provider 至少需要填写一个模型".to_string(),
        ));
    }
    if models.is_empty() {
        models.push(default_model.clone());
    }

    if !is_safe_id(&id) {
        return Err(AppError::InvalidInput(
            "Provider ID 只能包含小写字母、数字和短横线，长度 1-64".to_string(),
        ));
    }
    if name.is_empty() {
        return Err(AppError::InvalidInput("Provider 名称不能为空".to_string()));
    }
    if !(base_url.starts_with("http://") || base_url.starts_with("https://")) {
        return Err(AppError::InvalidInput(
            "Provider Base URL 必须以 http:// 或 https:// 开头".to_string(),
        ));
    }

    Ok(ProviderSettings {
        id,
        name,
        base_url,
        enabled: input.enabled,
        models,
        default_model,
        protocol: normalize_protocol(input.protocol.as_deref().unwrap_or(""))?,
    })
}

/// 支持的协议（第一版只实现 openai-chat，其余仅保留字段，调用时明确报错）
pub const PROTOCOLS: [&str; 3] = ["openai-chat", "openai-responses", "anthropic-messages"];

fn normalize_protocol(raw: &str) -> Result<String> {
    let value = raw.trim().to_lowercase();
    if value.is_empty() {
        return Ok("openai-chat".to_string());
    }
    if PROTOCOLS.contains(&value.as_str()) {
        Ok(value)
    } else {
        Err(AppError::InvalidInput(format!(
            "不支持的协议：{raw}（可选：{}）",
            PROTOCOLS.join(" / ")
        )))
    }
}

/// 预设里的单个模型（能力与推理档位随版本更新）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetModel {
    pub id: String,
    #[serde(default)]
    pub display_name: String,
    #[serde(default)]
    pub context_window: Option<u32>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub recommended: bool,
    #[serde(default)]
    pub reasoning: Option<PresetReasoning>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetReasoning {
    /// 请求里承载推理档位的字段名，例如 reasoning_effort
    #[serde(default)]
    pub param: String,
    #[serde(default)]
    pub levels: Vec<String>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetFile {
    #[serde(default)]
    pub version: u32,
    #[serde(default)]
    pub providers: Vec<ProviderPreset>,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ProviderPreset {
    pub id: String,
    pub name: String,
    #[serde(default)]
    pub group: String,
    pub base_url: String,
    #[serde(default)]
    pub protocol: String,
    #[serde(default)]
    pub note: String,
    #[serde(default)]
    pub models: Vec<PresetModel>,
    /// 同一供应商的多个接入地址（如火山 Coding 套餐与按量计费）
    #[serde(default)]
    pub endpoints: Vec<PresetEndpoint>,
    /// 小分类：cn = 国内，global = 海外
    #[serde(default)]
    pub region: String,
    /// true = 我方未实测，仅按官方文档填写
    #[serde(default)]
    pub unverified: bool,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PresetEndpoint {
    pub label: String,
    pub base_url: String,
    /// true = 界面只读（只有一个正确答案）
    #[serde(default)]
    pub locked: bool,
}

/// 读取预设文件版本号（缺失时回退 0）
pub fn presets_version(app_root: &Path) -> u32 {
    let path = app_root.join("config").join("providers.json");
    std::fs::read_to_string(&path)
        .ok()
        .and_then(|text| serde_json::from_str::<PresetFile>(&text).ok())
        .map(|file| file.version)
        .unwrap_or(0)
}

/// 从 `config/providers.json` 读取预设（随版本更新），缺失或损坏时回退内置清单
pub fn presets_from(app_root: &Path) -> Vec<ProviderPreset> {
    let path = app_root.join("config").join("providers.json");
    if let Ok(text) = std::fs::read_to_string(&path) {
        match serde_json::from_str::<PresetFile>(&text) {
            Ok(file) if !file.providers.is_empty() => {
                tracing::info!(
                    version = file.version,
                    providers = file.providers.len(),
                    "provider presets loaded"
                );
                return file.providers;
            }
            Ok(_) => {
                tracing::warn!(path = %path.display(), "provider presets empty, using builtin")
            }
            Err(err) => {
                tracing::warn!(path = %path.display(), error = %err, "provider presets parse failed")
            }
        }
    }
    presets()
}

fn preset(id: &str, name: &str, group: &str, base_url: &str, note: &str) -> ProviderPreset {
    ProviderPreset {
        id: id.to_string(),
        name: name.to_string(),
        group: group.to_string(),
        base_url: base_url.to_string(),
        protocol: "openai-chat".to_string(),
        note: note.to_string(),
        models: Vec::new(),
        endpoints: Vec::new(),
        region: String::new(),
        unverified: false,
    }
}

/// 内置预设。只列可验证的组合：DeepSeek 官方、OpenCode Zen、火山方舟、自定义网关。
pub fn presets() -> Vec<ProviderPreset> {
    vec![
        preset(
            "deepseek",
            "DeepSeek（官方直连）",
            "official",
            "https://api.deepseek.com/v1",
            "官方 OpenAI 兼容接口；模型通常为 deepseek-chat / deepseek-reasoner",
        ),
        preset(
            "opencode-zen",
            "OpenCode Zen（聚合网关）",
            "gateway",
            "https://opencode.ai/zen/v1",
            "模型目录公开可自动获取；GPT/Claude 走不同协议，本版仅支持 chat/completions 子集",
        ),
        preset(
            "volcano-ark",
            "火山方舟 Coding（聚合网关）",
            "gateway",
            "https://ark.cn-beijing.volces.com/api/coding/v3",
            "按套餐计费；模型名以控制台为准",
        ),
        preset(
            "custom",
            "自定义 OpenAI 兼容网关",
            "gateway",
            "",
            "Base URL 以 /v1 结尾时自动直接拼 /models 与 /chat/completions",
        ),
    ]
}

pub fn view(
    provider: &ProviderSettings,
    has_api_key: bool,
    reachable: Option<bool>,
) -> ProviderView {
    ProviderView {
        id: provider.id.clone(),
        name: provider.name.clone(),
        base_url: provider.base_url.clone(),
        enabled: provider.enabled,
        models: provider.models.clone(),
        default_model: provider.default_model.clone(),
        has_api_key,
        reachable,
    }
}

pub fn read_export_files(paths: &[String]) -> Vec<(String, String)> {
    let mut result = Vec::new();
    let mut seen = std::collections::HashSet::new();
    let mut total = 0usize;

    for path in paths {
        if result.len() >= MAX_EXPORT_FILES {
            break;
        }
        let path = path.trim();
        if path.is_empty() || !seen.insert(path.to_string()) {
            continue;
        }
        let file = match std::fs::File::open(path) {
            Ok(file) => file,
            Err(err) => {
                tracing::warn!(path, error = %err, "skip unreadable export file");
                continue;
            }
        };
        let mut buffer = Vec::new();
        if let Err(err) = file
            .take(MAX_EXPORT_FILE_BYTES as u64)
            .read_to_end(&mut buffer)
        {
            tracing::warn!(path, error = %err, "read export file failed");
            continue;
        }
        if total + buffer.len() > MAX_EXPORT_TOTAL_BYTES {
            break;
        }
        total += buffer.len();
        result.push((
            path.to_string(),
            String::from_utf8_lossy(&buffer).to_string(),
        ));
    }

    result
}

/// 展开错误链，便于定位是 DNS、TLS 还是连接被拒
fn error_chain(err: &dyn std::error::Error) -> String {
    let mut parts = vec![err.to_string()];
    let mut current = err.source();
    while let Some(next) = current {
        parts.push(next.to_string());
        current = next.source();
    }
    parts.join(" <- ")
}

fn endpoint(base_url: &str, suffix: &str) -> String {
    format!(
        "{}/{}",
        base_url.trim_end_matches('/'),
        suffix.trim_start_matches('/')
    )
}

/// 解析模型列表：兼容 OpenAI `{data:[{id}]}`、`{models:[...]}`、裸数组三种形态。
fn parse_model_list(body: &serde_json::Value) -> Vec<String> {
    let items = body
        .get("data")
        .and_then(serde_json::Value::as_array)
        .or_else(|| body.get("models").and_then(serde_json::Value::as_array))
        .or_else(|| body.as_array());

    let mut models: Vec<String> = Vec::new();
    for item in items.into_iter().flatten() {
        let id = item
            .get("id")
            .and_then(serde_json::Value::as_str)
            .or_else(|| item.get("name").and_then(serde_json::Value::as_str))
            .or_else(|| item.as_str());
        if let Some(id) = id {
            let id = id.trim();
            if !id.is_empty() {
                models.push(id.to_string());
            }
        }
    }
    models.sort();
    models.dedup();
    models
}

fn extract_content(value: &serde_json::Value) -> Option<String> {
    let message = value.pointer("/choices/0/message/content");
    if let Some(content) = message.and_then(|value| value.as_str()) {
        return Some(content.to_string());
    }
    if let Some(parts) = message.and_then(|value| value.as_array()) {
        let text = parts
            .iter()
            .filter_map(|part| part.get("text").and_then(|value| value.as_str()))
            .collect::<Vec<_>>()
            .join("");
        if !text.is_empty() {
            return Some(text);
        }
    }
    value
        .pointer("/choices/0/text")
        .and_then(|value| value.as_str())
        .map(str::to_string)
}

fn extract_usage(value: &serde_json::Value) -> Option<TokenUsage> {
    let usage = value.get("usage")?;
    let prompt_tokens = usage
        .get("prompt_tokens")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let completion_tokens = usage
        .get("completion_tokens")
        .and_then(|value| value.as_u64())
        .unwrap_or(0);
    let total_tokens = usage
        .get("total_tokens")
        .and_then(|value| value.as_u64())
        .unwrap_or(prompt_tokens + completion_tokens);
    Some(TokenUsage {
        prompt_tokens,
        completion_tokens,
        total_tokens,
    })
}

fn truncate(value: &str, limit: usize) -> String {
    value.chars().take(limit).collect()
}

fn is_safe_id(value: &str) -> bool {
    let length = value.chars().count();
    (1..=64).contains(&length)
        && value
            .chars()
            .all(|ch| ch.is_ascii_lowercase() || ch.is_ascii_digit() || ch == '-')
}

#[cfg(test)]
mod tests {
    use super::*;

    fn provider(id: &str) -> ProviderSettings {
        ProviderSettings {
            id: id.to_string(),
            name: "DeepSeek".to_string(),
            base_url: "https://api.deepseek.com/v1".to_string(),
            enabled: true,
            models: vec!["deepseek-chat".to_string()],
            default_model: "deepseek-chat".to_string(),
            protocol: "openai-chat".to_string(),
        }
    }

    #[test]
    fn normalizes_default_model_from_models() {
        let input = ProviderInput {
            id: "deepseek".to_string(),
            name: "DeepSeek".to_string(),
            base_url: "https://api.deepseek.com/v1/".to_string(),
            enabled: true,
            models: vec![" deepseek-chat ".to_string()],
            default_model: String::new(),
            api_key: None,
            protocol: None,
        };
        let settings = normalize(input).unwrap();
        assert_eq!(settings.base_url, "https://api.deepseek.com/v1");
        assert_eq!(settings.default_model, "deepseek-chat");
    }

    #[test]
    fn rejects_invalid_provider_url() {
        let input = ProviderInput {
            id: "relay".to_string(),
            name: "Relay".to_string(),
            base_url: "relay.local".to_string(),
            enabled: true,
            models: vec!["x".to_string()],
            default_model: "x".to_string(),
            api_key: None,
            protocol: None,
        };
        assert!(normalize(input).is_err());
    }

    #[test]
    fn defaults_protocol_to_openai_chat() {
        let input = ProviderInput {
            id: "zen".to_string(),
            name: "OpenCode Zen".to_string(),
            base_url: "https://opencode.ai/zen/v1".to_string(),
            enabled: true,
            models: vec!["deepseek-v4.1-flash".to_string()],
            default_model: "deepseek-v4.1-flash".to_string(),
            api_key: None,
            protocol: None,
        };
        let settings = normalize(input).expect("normalize");
        assert_eq!(settings.protocol, "openai-chat");
    }

    #[test]
    fn rejects_unknown_protocol() {
        let input = ProviderInput {
            id: "zen".to_string(),
            name: "Zen".to_string(),
            base_url: "https://opencode.ai/zen/v1".to_string(),
            enabled: true,
            models: vec!["m".to_string()],
            default_model: "m".to_string(),
            api_key: None,
            protocol: Some("grpc".to_string()),
        };
        assert!(normalize(input).is_err());
    }

    #[test]
    fn parses_openai_model_list() {
        let body = serde_json::json!({"object":"list","data":[{"id":"b"},{"id":"a"},{"id":"a"}]});
        assert_eq!(
            parse_model_list(&body),
            vec!["a".to_string(), "b".to_string()]
        );
    }

    #[test]
    fn parses_alternate_model_list_shapes() {
        let nested = serde_json::json!({"models":[{"name":"x"}]});
        assert_eq!(parse_model_list(&nested), vec!["x".to_string()]);
        let bare = serde_json::json!(["m1", "m2"]);
        assert_eq!(
            parse_model_list(&bare),
            vec!["m1".to_string(), "m2".to_string()]
        );
    }

    #[test]
    fn presets_cover_official_and_gateway() {
        let presets = presets();
        assert!(presets.iter().any(|item| item.group == "official"));
        assert!(presets.iter().any(|item| item.group == "gateway"));
        assert!(presets
            .iter()
            .any(|item| item.base_url == "https://opencode.ai/zen/v1"));
    }

    #[test]
    fn extracts_openai_style_usage() {
        let value = serde_json::json!({
            "choices": [{"message": {"content": "hello"}}],
            "usage": {"prompt_tokens": 3, "completion_tokens": 5, "total_tokens": 8}
        });
        assert_eq!(extract_content(&value).as_deref(), Some("hello"));
        assert_eq!(extract_usage(&value).unwrap().total_tokens, 8);
    }

    #[test]
    fn builds_chat_endpoint_without_double_slash() {
        assert_eq!(
            endpoint("https://api.deepseek.com/v1/", "chat/completions"),
            "https://api.deepseek.com/v1/chat/completions"
        );
    }

    #[test]
    fn provider_view_masks_key() {
        let view = view(&provider("deepseek"), true, Some(true));
        assert!(view.has_api_key);
        assert_eq!(view.default_model, "deepseek-chat");
    }

    #[test]
    fn export_files_caps_count_and_truncates_large_files() {
        let dir = std::env::temp_dir().join(format!(
            "ai-assistant-export-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_millis())
                .unwrap_or_default()
        ));
        std::fs::create_dir_all(&dir).unwrap();

        let mut paths = Vec::new();
        for index in 0..7 {
            let path = dir.join(format!("file-{index}.txt"));
            std::fs::write(&path, format!("content-{index}")).unwrap();
            paths.push(path.display().to_string());
        }
        let big = dir.join("big.txt");
        std::fs::write(&big, "a".repeat(80 * 1024)).unwrap();
        paths.push(big.display().to_string());

        let files = read_export_files(&paths);
        assert!(files.len() <= MAX_EXPORT_FILES);
        let total = files
            .iter()
            .map(|(_, content)| content.len())
            .sum::<usize>();
        assert!(total <= MAX_EXPORT_TOTAL_BYTES);
        for (_, content) in &files {
            assert!(content.len() <= MAX_EXPORT_FILE_BYTES);
        }
        std::fs::remove_dir_all(&dir).unwrap();
    }
}
