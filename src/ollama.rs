use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};
use std::time::{Duration, Instant};

#[derive(Clone)]
pub struct OllamaClient {
    base_url: String,
    http: reqwest::Client,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    #[serde(default)]
    pub content: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tool_calls: Option<Vec<ToolCall>>,
}

impl ChatMessage {
    fn base(role: &str, content: impl Into<String>) -> Self {
        Self {
            role: role.to_string(),
            content: content.into(),
            tool_calls: None,
        }
    }

    pub fn system(content: impl Into<String>) -> Self {
        Self::base("system", content)
    }

    pub fn user(content: impl Into<String>) -> Self {
        Self::base("user", content)
    }

    pub fn assistant(content: impl Into<String>) -> Self {
        Self::base("assistant", content)
    }

    pub fn tool(content: impl Into<String>) -> Self {
        Self::base("tool", content)
    }

    pub fn assistant_with_tools(content: impl Into<String>, tool_calls: Vec<ToolCall>) -> Self {
        Self {
            role: "assistant".to_string(),
            content: content.into(),
            tool_calls: Some(tool_calls),
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCall {
    pub function: ToolCallFunction,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolCallFunction {
    pub name: String,
    #[serde(default)]
    pub arguments: serde_json::Value,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    #[serde(rename = "type")]
    pub kind: String,
    pub function: ToolFunctionDefinition,
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolFunctionDefinition {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

impl ToolDefinition {
    pub fn new(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: serde_json::Value,
    ) -> Self {
        Self {
            kind: "function".to_string(),
            function: ToolFunctionDefinition {
                name: name.into(),
                description: description.into(),
                parameters,
            },
        }
    }
}

#[derive(Debug, Clone)]
pub struct ChatResult {
    pub message: ChatMessage,
    pub elapsed_ms: u64,
}

#[derive(Serialize)]
struct ChatRequest<'a> {
    model: &'a str,
    messages: &'a [ChatMessage],
    stream: bool,
    keep_alive: &'a str,
    options: ChatOptions,
    #[serde(skip_serializing_if = "Option::is_none")]
    format: Option<&'a str>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tools: Option<&'a [ToolDefinition]>,
}

#[derive(Debug, Clone, Copy, Serialize)]
pub struct ChatOptions {
    pub temperature: f32,
    pub num_ctx: u32,
    pub num_predict: u32,
}

impl Default for ChatOptions {
    fn default() -> Self {
        Self {
            temperature: 0.3,
            num_ctx: 4096,
            num_predict: 512,
        }
    }
}

#[derive(Deserialize)]
struct ChatResponse {
    message: ChatMessage,
}

#[derive(Deserialize)]
struct TagsResponse {
    #[serde(default)]
    models: Vec<TagModel>,
}

#[derive(Deserialize)]
struct TagModel {
    #[serde(default)]
    name: String,
}

impl OllamaClient {
    pub fn new(base_url: impl Into<String>) -> Result<Self> {
        let mut base_url = base_url.into().trim().trim_end_matches('/').to_string();
        if !base_url.contains("://") {
            base_url = format!("http://{base_url}");
        }
        let http = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(2))
            .build()
            .map_err(|err| AppError::Ollama(format!("初始化 HTTP 客户端失败：{err}")))?;
        Ok(Self { base_url, http })
    }

    pub fn base_url(&self) -> &str {
        &self.base_url
    }

    pub async fn list_models(&self, timeout: Duration) -> Result<Vec<String>> {
        let url = format!("{}/api/tags", self.base_url);
        let future = self.http.get(&url).send();
        let response = tokio::time::timeout(timeout, future)
            .await
            .map_err(|_| AppError::Timeout(timeout.as_secs()))?
            .map_err(|err| AppError::Ollama(format!("连接 {url} 失败：{err}")))?;

        if !response.status().is_success() {
            return Err(AppError::Ollama(format!("HTTP {}", response.status())));
        }

        let body: TagsResponse = response
            .json()
            .await
            .map_err(|err| AppError::Ollama(format!("解析模型列表失败：{err}")))?;

        Ok(body.models.into_iter().map(|model| model.name).collect())
    }

    pub async fn chat(
        &self,
        model: &str,
        messages: &[ChatMessage],
        timeout: Duration,
    ) -> Result<(String, u64)> {
        self.chat_with(model, messages, timeout, ChatOptions::default(), None)
            .await
    }

    pub async fn chat_with(
        &self,
        model: &str,
        messages: &[ChatMessage],
        timeout: Duration,
        options: ChatOptions,
        format: Option<&str>,
    ) -> Result<(String, u64)> {
        let result = self
            .send(model, messages, timeout, options, format, None)
            .await?;
        Ok((result.message.content, result.elapsed_ms))
    }

    pub async fn chat_tools(
        &self,
        model: &str,
        messages: &[ChatMessage],
        timeout: Duration,
        options: ChatOptions,
        tools: &[ToolDefinition],
    ) -> Result<ChatResult> {
        self.send(model, messages, timeout, options, None, Some(tools))
            .await
    }

    async fn send(
        &self,
        model: &str,
        messages: &[ChatMessage],
        timeout: Duration,
        options: ChatOptions,
        format: Option<&str>,
        tools: Option<&[ToolDefinition]>,
    ) -> Result<ChatResult> {
        let url = format!("{}/api/chat", self.base_url);
        let request = ChatRequest {
            model,
            messages,
            stream: false,
            keep_alive: "10m",
            options,
            format,
            tools,
        };

        let prompt_chars: usize = messages.iter().map(|m| m.content.chars().count()).sum();
        tracing::info!(
            model,
            messages = messages.len(),
            prompt_chars,
            num_predict = options.num_predict,
            json = format.is_some(),
            tools = tools.map(|tools| tools.len()).unwrap_or(0),
            url = %url,
            "ollama chat request"
        );

        let started = Instant::now();
        let future = self.http.post(&url).json(&request).send();
        let response = match tokio::time::timeout(timeout, future).await {
            Ok(Ok(response)) => response,
            Ok(Err(err)) => {
                tracing::warn!(model, error = %err, "ollama chat transport error");
                return Err(AppError::Ollama(format!("请求 Ollama 失败：{err}")));
            }
            Err(_) => {
                tracing::warn!(
                    model,
                    timeout_ms = timeout.as_millis() as u64,
                    "ollama chat timeout"
                );
                return Err(AppError::Timeout(timeout.as_secs()));
            }
        };

        let status = response.status();
        if !status.is_success() {
            let body = response.text().await.unwrap_or_default();
            tracing::warn!(model, %status, body = %body, "ollama chat http error");
            return Err(AppError::Ollama(format!("HTTP {status}：{body}")));
        }

        let body: ChatResponse = response
            .json()
            .await
            .map_err(|err| AppError::Ollama(format!("解析回复失败：{err}")))?;

        let elapsed_ms = started.elapsed().as_millis() as u64;
        let tool_call_count = body
            .message
            .tool_calls
            .as_ref()
            .map(|calls| calls.len())
            .unwrap_or(0);
        tracing::info!(
            model,
            elapsed_ms,
            reply_chars = body.message.content.chars().count(),
            tool_calls = tool_call_count,
            "ollama chat response"
        );

        Ok(ChatResult {
            message: body.message,
            elapsed_ms,
        })
    }

    pub async fn preload(&self, model: &str, num_ctx: u32, timeout: Duration) -> Result<u64> {
        let messages = [ChatMessage::user("hi")];
        let options = ChatOptions {
            temperature: 0.0,
            num_ctx,
            num_predict: 1,
        };
        let (_, elapsed_ms) = self
            .chat_with(model, &messages, timeout, options, None)
            .await?;
        Ok(elapsed_ms)
    }
}
