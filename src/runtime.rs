use crate::config::{self, AppConfig, ModelsConfig};
use crate::coordinator::{self, ToolTrace};
use crate::environment::{self, EnvironmentReport};
use crate::error::{AppError, Result};
use crate::memory::{LongTermFact, LongTermMemory, SessionMessage, SessionStore};
use crate::ollama::OllamaClient;
use crate::orchestrator::{self, OrchestrationContext};
use crate::permissions::PermissionPrompt;
use crate::progress::ProgressSink;
use crate::roles;
use crate::router::RouterDecision;
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::time::Duration;
use tracing_appender::non_blocking::WorkerGuard;

pub struct Runtime {
    pub app_root: PathBuf,
    pub data_root: PathBuf,
    pub config: AppConfig,
    pub models: ModelsConfig,
    pub ollama: OllamaClient,
    session: SessionStore,
    memory: LongTermMemory,
    _log_guard: WorkerGuard,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AppStatus {
    pub app_root: String,
    pub data_dir: String,
    pub router_model: String,
    pub coordinator_model: String,
    pub ollama_base_url: String,
    pub ollama_ready: bool,
    pub coordinator_ready: bool,
    pub installed_models: Vec<String>,
    #[serde(default)]
    pub missing_models: Vec<String>,
    pub agents: u32,
    pub pending_confirm: u32,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ChatReply {
    pub role: String,
    pub model: String,
    pub content: String,
    pub elapsed_ms: u64,
    #[serde(default)]
    pub tool_calls: Vec<ToolTrace>,
}

pub fn bootstrap() -> Result<Runtime> {
    let app_root = config::app_root();
    let data_root = crate::data::root(&app_root);
    crate::data::ensure_layout(&data_root)?;

    let (config, models) = config::load(&app_root)?;
    let log_guard = crate::logging::init(&data_root)?;

    let host = std::env::var("OLLAMA_HOST")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| "http://127.0.0.1:11434".to_string());
    let ollama = OllamaClient::new(host)?;

    tracing::info!(
        app = %config.app.name,
        version = %config.app.version,
        app_root = %app_root.display(),
        data_dir = %data_root.display(),
        router_model = %config.router.model,
        coordinator_model = %config.coordinator.model,
        ollama = %ollama.base_url(),
        "ai-assistant bootstrap complete"
    );

    let session = SessionStore::new(&data_root);
    let memory = LongTermMemory::new(&data_root);

    Ok(Runtime {
        app_root,
        data_root,
        config,
        models,
        ollama,
        session,
        memory,
        _log_guard: log_guard,
    })
}

impl Runtime {
    pub fn data_path(&self, relative: impl AsRef<Path>) -> PathBuf {
        self.data_root.join(relative)
    }

    pub async fn status(&self) -> AppStatus {
        let installed_models = self
            .ollama
            .list_models(Duration::from_millis(1500))
            .await
            .unwrap_or_default();
        let missing_models = environment::required_models(&self.config)
            .into_iter()
            .filter(|model| !environment::model_available(&installed_models, model))
            .collect::<Vec<_>>();

        AppStatus {
            app_root: self.app_root.display().to_string(),
            data_dir: self.data_root.display().to_string(),
            router_model: self.config.router.model.clone(),
            coordinator_model: self.config.coordinator.model.clone(),
            ollama_base_url: self.ollama.base_url().to_string(),
            ollama_ready: !installed_models.is_empty() && missing_models.is_empty(),
            coordinator_ready: environment::model_available(
                &installed_models,
                &self.config.coordinator.model,
            ),
            installed_models,
            missing_models,
            agents: 0,
            pending_confirm: 0,
        }
    }

    pub async fn environment(&self) -> EnvironmentReport {
        environment::check(&self.ollama, &self.config).await
    }

    pub async fn route(&self, content: &str) -> RouterDecision {
        crate::router::route(&self.ollama, &self.config, content).await
    }

    pub async fn warmup_router(&self) {
        let model = &self.config.router.model;
        match self
            .ollama
            .preload(model, 4096, Duration::from_secs(60))
            .await
        {
            Ok(elapsed_ms) => {
                tracing::info!(model = %model, elapsed_ms, "router warmup complete")
            }
            Err(err) => tracing::warn!(model = %model, error = %err, "router warmup skipped"),
        }
    }

    pub async fn warmup_coordinator(&self) {
        let model = &self.config.coordinator.model;
        let num_ctx = self.config.coordinator.max_tokens.max(2048);
        match self
            .ollama
            .preload(model, num_ctx, Duration::from_secs(180))
            .await
        {
            Ok(elapsed_ms) => {
                tracing::info!(model = %model, elapsed_ms, "coordinator warmup complete")
            }
            Err(err) => {
                tracing::warn!(model = %model, error = %err, "coordinator warmup skipped")
            }
        }
    }

    pub async fn chat(
        &self,
        role: &str,
        message: &str,
        prompt: Option<&dyn PermissionPrompt>,
    ) -> Result<ChatReply> {
        self.chat_with_progress(role, message, prompt, None).await
    }

    pub async fn chat_with_progress(
        &self,
        role: &str,
        message: &str,
        prompt: Option<&dyn PermissionPrompt>,
        progress: Option<&dyn ProgressSink>,
    ) -> Result<ChatReply> {
        if !roles::is_valid(role) {
            return Err(AppError::InvalidInput(format!("未知角色：{role}")));
        }

        let message = message.trim();
        if message.is_empty() {
            return Err(AppError::InvalidInput("消息不能为空".to_string()));
        }

        if let Err(err) = self.memory.remember_from_user(role, message) {
            tracing::warn!(role, error = %err, "long-term memory update failed");
        }

        let history = self.session.load(role);
        let model = self.config.coordinator.model.clone();
        let orchestrate = orchestrator::should_orchestrate(message);
        tracing::info!(
            role,
            model = %model,
            history_messages = history.len(),
            orchestrate,
            "chat requested"
        );

        let (content, model, elapsed_ms, tool_calls) = if orchestrate {
            let context = OrchestrationContext {
                ollama: &self.ollama,
                config: &self.config,
                app_root: &self.app_root,
                data_root: &self.data_root,
                prompt,
                progress,
            };
            let reply = orchestrator::run(&context, role, message, &history).await?;
            let tool_calls = reply
                .subtask_results
                .iter()
                .filter_map(|result| result.tool.clone())
                .collect();
            (reply.content, reply.model, reply.elapsed_ms, tool_calls)
        } else {
            let context = coordinator::CoordinatorContext {
                ollama: &self.ollama,
                config: &self.config,
                app_root: &self.app_root,
                data_root: &self.data_root,
                prompt,
            };
            let reply = coordinator::run(&context, role, message, &history).await?;
            (
                reply.content,
                reply.model,
                reply.elapsed_ms,
                reply.tool_calls,
            )
        };

        if let Err(err) = self.session.append(role, message, &content) {
            tracing::warn!(role, error = %err, "persist session failed");
        }

        Ok(ChatReply {
            role: role.to_string(),
            model,
            content,
            elapsed_ms,
            tool_calls,
        })
    }

    pub fn history(&self, role: &str) -> Vec<SessionMessage> {
        self.session.load(role)
    }

    pub fn memory_facts(&self) -> Vec<LongTermFact> {
        self.memory.facts()
    }

    pub fn clear_history(&self, role: &str) -> Result<()> {
        self.session.clear(role)
    }
}
