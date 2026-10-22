use crate::config::AppConfig;
use crate::error::AppError;
use crate::ollama::{ChatMessage, ChatOptions, OllamaClient};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::time::{Duration, Instant};

pub const FALLBACK_ROLE: &str = "general";
pub const FALLBACK_INTENT: &str = "unknown";

pub const EVAL_SAMPLES: [(&str, &str); 10] = [
    ("帮我把 src/main.rs 里的第 10 行改一下", "dev"),
    ("解释一下 Rust 的生命周期", "dev"),
    ("今天天气怎么样", "general"),
    ("给我讲个笑话", "general"),
    ("切换到通用角色", "general"),
    ("把这个项目里所有 TODO 找出来", "dev"),
    ("你好", "general"),
    ("帮我写一个 Python 快速排序", "dev"),
    ("你最近怎么样", "general"),
    ("看看当前目录有什么文件", "dev"),
];

const ROUTER_SYSTEM_PROMPT: &str = r#"你是“小明同学”的路由分类器，只负责判断用户消息应由哪个角色处理。
只输出一个 JSON 对象，不要输出解释、Markdown、代码块或多余文字。

role 只能是以下三个值之一：
- "dev"：软件工程相关。包括写代码、读代码、改代码、解释编程概念或语言特性、文件操作、终端命令、Git、调试、测试、项目结构。
- "general"：日常对话与通用问答。包括闲聊、问候、通用知识、写作、翻译、生活建议、与软件工程无关的问题。
- "custom-role"：用户明确要求垂直领域专家，或问题属于法律、医疗、财务等专业咨询。

intent 只能是：chat、task、role_switch、config、knowledge、system、multimodal、unknown。
confidence 是 0 到 1 之间的小数，表示你对 role 判断的置信度。

示例：
用户：帮我把 main.rs 里的函数改一下
输出：{"role":"dev","intent":"task","confidence":0.96}
用户：讲个笑话
输出：{"role":"general","intent":"chat","confidence":0.97}
用户：切换到通用角色
输出：{"role":"general","intent":"role_switch","confidence":0.95}
用户：解释一下 Rust 的生命周期
输出：{"role":"dev","intent":"knowledge","confidence":0.93}"#;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouterDecision {
    pub role: String,
    pub intent: String,
    pub confidence: f64,
    pub fallback: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
    pub model: String,
    pub elapsed_ms: u64,
}

#[derive(Debug, Deserialize)]
struct RawDecision {
    #[serde(default, alias = "suggested_role", alias = "suggestedRole")]
    role: Option<String>,
    #[serde(default)]
    intent: Option<String>,
    #[serde(default)]
    confidence: Option<Value>,
}

pub async fn route(ollama: &OllamaClient, config: &AppConfig, content: &str) -> RouterDecision {
    let started = Instant::now();
    let model = config.router.model.clone();
    let text = content.trim();

    if text.is_empty() {
        return fallback(&model, 0, 0.0, "empty_input");
    }

    let max_chars = (config.router.max_input_tokens as usize)
        .saturating_mul(2)
        .clamp(256, 6000);
    let text: String = text.chars().take(max_chars).collect();
    let input_chars = text.chars().count();

    let timeout = Duration::from_millis(config.router.timeout_ms.max(1));
    let options = ChatOptions {
        temperature: 0.1,
        num_ctx: config.router.max_input_tokens.max(2048),
        num_predict: config.router.max_output_tokens.max(32),
    };
    let messages = [
        ChatMessage::system(ROUTER_SYSTEM_PROMPT),
        ChatMessage::user(text),
    ];

    tracing::info!(
        model = %model,
        input_chars,
        timeout_ms = config.router.timeout_ms,
        "router request"
    );

    let result = ollama
        .chat_with(&model, &messages, timeout, options, Some("json"))
        .await;
    let elapsed_ms = started.elapsed().as_millis() as u64;

    match result {
        Ok((raw, _)) => {
            tracing::debug!(output = %truncate(&raw, 200), "router raw output");
            parse_decision(&raw, &model, elapsed_ms, config.router.confidence_threshold)
        }
        Err(AppError::Timeout(_)) => {
            tracing::warn!(model = %model, elapsed_ms, "router timeout");
            fallback(&model, elapsed_ms, 0.0, "timeout")
        }
        Err(err) => {
            tracing::warn!(model = %model, elapsed_ms, error = %err, "router request failed");
            fallback(&model, elapsed_ms, 0.0, "ollama_error")
        }
    }
}

fn parse_decision(raw: &str, model: &str, elapsed_ms: u64, threshold: f64) -> RouterDecision {
    let Some(json) = extract_json(raw) else {
        tracing::warn!(output = %truncate(raw, 200), "router output is not json");
        return fallback(model, elapsed_ms, 0.0, "invalid_json");
    };

    let parsed: RawDecision = match serde_json::from_str(json) {
        Ok(parsed) => parsed,
        Err(err) => {
            tracing::warn!(error = %err, output = %truncate(raw, 200), "router json parse failed");
            return fallback(model, elapsed_ms, 0.0, "invalid_json");
        }
    };

    let intent = parsed
        .intent
        .as_deref()
        .map(normalize_intent)
        .unwrap_or(FALLBACK_INTENT);

    let Some(confidence) = parsed.confidence.as_ref().and_then(parse_confidence) else {
        return fallback(model, elapsed_ms, 0.0, "missing_confidence");
    };

    let role = match parsed.role.as_deref().and_then(normalize_role) {
        Some(role) => role,
        None => {
            let mut decision = fallback(model, elapsed_ms, confidence, "invalid_role");
            decision.intent = intent.to_string();
            return decision;
        }
    };

    if confidence < threshold {
        let mut decision = fallback(model, elapsed_ms, confidence, "low_confidence");
        decision.intent = intent.to_string();
        return decision;
    }

    let decision = RouterDecision {
        role: role.to_string(),
        intent: intent.to_string(),
        confidence,
        fallback: false,
        reason: None,
        model: model.to_string(),
        elapsed_ms,
    };

    tracing::info!(
        model = %decision.model,
        elapsed_ms = decision.elapsed_ms,
        role = %decision.role,
        intent = %decision.intent,
        confidence = decision.confidence,
        "router decision"
    );

    decision
}

fn fallback(model: &str, elapsed_ms: u64, confidence: f64, reason: &str) -> RouterDecision {
    tracing::warn!(model, elapsed_ms, confidence, reason, "router fallback");
    RouterDecision {
        role: FALLBACK_ROLE.to_string(),
        intent: FALLBACK_INTENT.to_string(),
        confidence,
        fallback: true,
        reason: Some(reason.to_string()),
        model: model.to_string(),
        elapsed_ms,
    }
}

fn normalize_role(raw: &str) -> Option<&'static str> {
    match raw.trim().to_lowercase().as_str() {
        "dev" | "developer" | "开发" | "开发者" | "程序" => Some("dev"),
        "general" | "chat" | "通用" | "通用用户" | "普通" | "普通用户" | "日常" => {
            Some("general")
        }
        "custom-role" | "custom_role" | "customrole" | "vertical" | "垂直" | "垂直角色"
        | "专家" | "domain" => Some("custom-role"),
        _ => None,
    }
}

fn normalize_intent(raw: &str) -> &'static str {
    match raw.trim().to_lowercase().as_str() {
        "chat" | "闲聊" => "chat",
        "task" | "任务" => "task",
        "role_switch" | "role-switch" | "roleswitch" | "切换角色" => "role_switch",
        "config" | "配置" => "config",
        "knowledge" | "知识" => "knowledge",
        "system" | "系统" => "system",
        "multimodal" | "多模态" => "multimodal",
        _ => FALLBACK_INTENT,
    }
}

fn parse_confidence(value: &Value) -> Option<f64> {
    let number = match value {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => {
            let text = text.trim();
            let percent = text.ends_with('%');
            let cleaned = text.trim_end_matches('%').trim();
            cleaned
                .parse::<f64>()
                .ok()
                .map(|value| if percent { value / 100.0 } else { value })
        }
        _ => None,
    }?;

    if !number.is_finite() {
        return None;
    }
    if (0.0..=1.0).contains(&number) {
        Some(number)
    } else if number > 1.0 && number <= 100.0 {
        Some(number / 100.0)
    } else {
        None
    }
}

fn extract_json(text: &str) -> Option<&str> {
    let trimmed = text.trim();
    if trimmed.is_empty() {
        return None;
    }
    let start = trimmed.find('{')?;
    let end = trimmed.rfind('}')?;
    if end > start {
        Some(&trimmed[start..=end])
    } else {
        None
    }
}

fn truncate(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn parse(raw: &str, threshold: f64) -> RouterDecision {
        parse_decision(raw, "test-model", 12, threshold)
    }

    #[test]
    fn parses_plain_json() {
        let decision = parse(r#"{"role":"dev","intent":"task","confidence":0.92}"#, 0.5);
        assert_eq!(decision.role, "dev");
        assert_eq!(decision.intent, "task");
        assert!((decision.confidence - 0.92).abs() < 1e-9);
        assert!(!decision.fallback);
        assert!(decision.reason.is_none());
    }

    #[test]
    fn parses_fenced_json() {
        let decision = parse(
            "```json\n{\"role\":\"general\",\"intent\":\"chat\",\"confidence\":0.8}\n```",
            0.5,
        );
        assert_eq!(decision.role, "general");
        assert!(!decision.fallback);
    }

    #[test]
    fn low_confidence_falls_back_to_general() {
        let decision = parse(r#"{"role":"dev","intent":"task","confidence":0.3}"#, 0.5);
        assert_eq!(decision.role, FALLBACK_ROLE);
        assert_eq!(decision.intent, "task");
        assert!(decision.fallback);
        assert_eq!(decision.reason.as_deref(), Some("low_confidence"));
        assert!((decision.confidence - 0.3).abs() < 1e-9);
    }

    #[test]
    fn invalid_or_missing_fields_fall_back() {
        let invalid_role = parse(r#"{"role":"wizard","confidence":0.9}"#, 0.5);
        assert_eq!(invalid_role.role, FALLBACK_ROLE);
        assert_eq!(invalid_role.reason.as_deref(), Some("invalid_role"));

        let missing = parse(r#"{"role":"dev","intent":"task"}"#, 0.5);
        assert_eq!(missing.role, FALLBACK_ROLE);
        assert_eq!(missing.reason.as_deref(), Some("missing_confidence"));

        let broken = parse("not json", 0.5);
        assert_eq!(broken.role, FALLBACK_ROLE);
        assert_eq!(broken.reason.as_deref(), Some("invalid_json"));
    }

    #[test]
    fn accepts_percentage_and_alias() {
        let decision = parse(
            r#"{"suggested_role":"general","intent":"chat","confidence":"87%"}"#,
            0.5,
        );
        assert_eq!(decision.role, "general");
        assert!((decision.confidence - 0.87).abs() < 1e-9);
        assert!(!decision.fallback);
    }
}
