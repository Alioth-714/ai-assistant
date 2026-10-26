use crate::audit;
use crate::capabilities::{self, Capabilities, Route, RouteDecision, TaskSignals};
use crate::config::{self, AppConfig, ModelsConfig};
use crate::coordinator::{self, ToolTrace};
use crate::environment::{self, EnvironmentReport};
use crate::error::{AppError, Result};
use crate::memory::{
    LongTermFact, LongTermMemory, SessionDetail, SessionMessage, SessionStore, SessionSummary,
    SessionTurn,
};
use crate::ollama::{ChatOptions, OllamaClient};
use crate::orchestrator::{self, OrchestrationContext};
use crate::permissions::PermissionPrompt;
use crate::planner;
use crate::progress::ProgressSink;
use crate::providers::{
    self, ExportGate, ExternalChatOutcome, ExternalClient, HealthEntry, ProviderInput,
    ProviderTest, ProviderView,
};
use crate::roles::{self, RoleSummary};
use crate::router::RouterDecision;
use crate::secrets;
use crate::settings::{self, ProviderSettings, SaveSettingsResult, SettingsView, UserSettings};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};
use tracing_appender::non_blocking::WorkerGuard;

pub struct Runtime {
    pub app_root: PathBuf,
    pub data_root: PathBuf,
    pub config: AppConfig,
    pub models: ModelsConfig,
    pub ollama: OllamaClient,
    session: SessionStore,
    memory: LongTermMemory,
    settings: RwLock<UserSettings>,
    providers: ExternalClient,
    provider_health: Mutex<HashMap<String, HealthEntry>>,
    /// 能力分层配置（决策 33）：profile 可热切换（small/large）
    capabilities: RwLock<Capabilities>,
    /// 会话云端预算（决策 33）：超额自动降级为仅本地
    cloud_budget: Mutex<crate::egress::CloudBudget>,
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
    pub session_id: String,
    #[serde(default)]
    pub tool_calls: Vec<ToolTrace>,
    /// 能力分层路由决策（决策 33）；前端据此提示"可申请云端协助"
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub route: Option<RouteDecision>,
}

/// 比较两个模型名是否等价（忽略 `:latest` 后缀与大小写）
fn same_model(left: &str, right: &str) -> bool {
    fn normalize(value: &str) -> String {
        value.trim().trim_end_matches(":latest").to_lowercase()
    }
    !right.trim().is_empty() && normalize(left) == normalize(right)
}

/// 从计划执行结果里取出最终答复（优先最后一步的 summary）。
fn planner_content(outputs: &[serde_json::Value]) -> String {
    for value in outputs.iter().rev() {
        if let Some(summary) = value.get("summary").and_then(|item| item.as_str()) {
            if !summary.trim().is_empty() {
                return summary.to_string();
            }
        }
        if let Some(err) = value.get("error").and_then(|item| item.as_str()) {
            return format!("总结失败：{err}");
        }
    }
    "已读取文件，但没有生成摘要。".to_string()
}

pub fn bootstrap() -> Result<Runtime> {
    let app_root = config::app_root();
    let data_root = crate::data::root(&app_root);
    crate::data::ensure_layout(&data_root)?;

    let (mut config, models) = config::load(&app_root)?;
    let user_settings = settings::load(&data_root, &config, &models);
    settings::apply_model_overrides(&user_settings, &mut config);
    let log_guard = crate::logging::init(&data_root)?;

    let host = std::env::var("OLLAMA_HOST")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .unwrap_or_else(|| user_settings.models.ollama_host.clone());
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
    let providers = ExternalClient::new()?;
    // 历史明文密钥升级为 DPAPI 密文（失败不影响启动）
    if let Err(err) = secrets::migrate(&data_root) {
        tracing::warn!(error = %err, "密钥加密迁移失败，保持原文件不动");
    }
    // 能力档位与预算从用户设置恢复（决策 33）
    let mut loaded_caps = capabilities::load(&app_root);
    if let Err(err) = loaded_caps.switch_profile(&user_settings.cloud.profile) {
        tracing::warn!(
            profile = %user_settings.cloud.profile,
            error = %err,
            "设置里的能力档位无效，使用配置文件默认档位"
        );
    }
    let budget_limit = if user_settings.cloud.budget_tokens == 0 {
        None
    } else {
        Some(user_settings.cloud.budget_tokens)
    };

    Ok(Runtime {
        app_root,
        data_root,
        config,
        models,
        ollama,
        session,
        memory,
        settings: RwLock::new(user_settings),
        providers,
        provider_health: Mutex::new(HashMap::new()),
        capabilities: RwLock::new(loaded_caps),
        cloud_budget: Mutex::new(crate::egress::CloudBudget::new(budget_limit)),
        _log_guard: log_guard,
    })
}

impl Runtime {
    /// 执行一个模板计划（决策 31）：读 → 单步总结，工具由代码编排、模型只做单步生成。
    async fn run_planner_template(
        &self,
        message: &str,
        template: planner::TemplateId,
        args: &serde_json::Value,
        prompt: Option<&dyn PermissionPrompt>,
        model: &str,
    ) -> Result<Vec<serde_json::Value>> {
        let mut plan = planner::build_plan(
            format!("plan_{}", crate::util::now_ms()),
            message,
            template,
            args,
        )?;
        let summarizer = planner::Summarizer {
            ollama: &self.ollama,
            model,
            options: ChatOptions {
                temperature: 0.2,
                num_ctx: 8192,
                num_predict: 800,
                seed: None,
            }
            .with_determinism(self.config.testing.deterministic),
            timeout: Duration::from_secs(90),
        };
        tracing::info!(
            template = template.as_str(),
            steps = plan.steps.len(),
            "planner template plan"
        );
        planner::run_plan(
            &mut plan,
            &self.app_root,
            &self.data_root,
            prompt,
            Some(summarizer),
        )
        .await
    }

    /// 能力分层路由（决策 33）：确定性判定，不调模型。
    /// 判定优先级：用户「仅本地」开关 > 静态能力白名单 > 确定性信号（文件数/输入量）。
    pub fn route_decision(&self, message: &str) -> RouteDecision {
        let local_only = !self.current_settings().cloud.auto_escalate;
        let capability = capabilities::classify(message);
        let signals = TaskSignals {
            files: orchestrator::extract_paths(message).len(),
            chars: message.chars().count(),
        };
        let caps = self
            .capabilities
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        capabilities::decide(&caps, local_only, capability, &signals)
    }

    /// 切换能力档位（small / large）：换硬件后只切 profile，代码与角色配置不动。
    pub fn switch_capability_profile(&self, profile: &str) -> Result<()> {
        let mut guard = self
            .capabilities
            .write()
            .map_err(|_| AppError::InvalidInput("能力配置不可用".to_string()))?;
        guard.switch_profile(profile)
    }

    /// 当前能力档位与能力表（供设置页展示）
    pub fn capabilities_view(&self) -> Capabilities {
        self.capabilities
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

    /// 应用云端偏好（决策 33）：预算上限 + 能力档位，保存设置后即时生效、无需重启。
    fn apply_cloud_preferences(&self, settings: &UserSettings) {
        let limit = if settings.cloud.budget_tokens == 0 {
            None
        } else {
            Some(settings.cloud.budget_tokens)
        };
        if let Ok(mut budget) = self.cloud_budget.lock() {
            budget.limit_tokens = limit;
        }
        if let Ok(mut caps) = self.capabilities.write() {
            if let Err(err) = caps.switch_profile(&settings.cloud.profile) {
                tracing::warn!(
                    profile = %settings.cloud.profile,
                    error = %err,
                    "能力档位切换失败，保持当前档位"
                );
            }
        }
    }

    /// 上云前的本地确定性抽取（决策 33）：目录树 / 符号表 / 摘要，零模型开销。
    fn cloud_digest(&self, message: &str) -> Option<String> {
        let paths = orchestrator::extract_paths(message);
        if paths.is_empty() {
            return None;
        }
        let digest = crate::extract::build_digest(
            &self.app_root,
            &paths,
            None,
            crate::extract::DigestOptions::default(),
        );
        if digest.trim().is_empty() {
            None
        } else {
            Some(digest)
        }
    }

    /// 数据目录迁移（5A）：校验目标 → 建目录 → 复制并校验 → 写指针；重启后生效。
    pub fn set_data_root(&self, target: &str) -> Result<crate::data::MigrationReport> {
        let target_text = target.trim();
        if target_text.is_empty() {
            return Err(AppError::InvalidInput("目标目录不能为空".to_string()));
        }
        let target_path = if Path::new(target_text).is_absolute() {
            PathBuf::from(target_text)
        } else {
            self.app_root.join(target_text)
        };
        let current = self.data_root.clone();
        if target_path == current {
            return Ok(crate::data::MigrationReport {
                source: current.display().to_string(),
                target: target_path.display().to_string(),
                files: 0,
                bytes: 0,
                verified: true,
                skipped: 0,
            });
        }

        crate::data::ensure_layout(&target_path)?;
        let report = crate::data::migrate(&current, &target_path)?;
        if !report.verified {
            return Err(AppError::InvalidInput(
                "迁移后校验未通过，指针未改动；请检查目标目录权限后重试".to_string(),
            ));
        }
        crate::data::write_pointer(&target_path)?;
        audit::event(
            &self.data_root,
            "data_root",
            "user",
            "migrate",
            "success",
            serde_json::json!({
                "target": target_path.display().to_string(),
                "files": report.files,
                "bytes": report.bytes,
                "skipped": report.skipped
            }),
        );
        tracing::info!(
            target = %target_path.display(),
            files = report.files,
            bytes = report.bytes,
            "data root migrated, restart required"
        );
        Ok(report)
    }

    /// 会话云端预算状态（已用 / 上限）
    pub fn cloud_budget(&self) -> crate::egress::CloudBudget {
        self.cloud_budget
            .lock()
            .map(|guard| guard.clone())
            .unwrap_or_default()
    }

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
        self.chat_with_progress(role, message, prompt, None, None)
            .await
    }

    pub async fn chat_with_progress(
        &self,
        role: &str,
        message: &str,
        prompt: Option<&dyn PermissionPrompt>,
        progress: Option<&dyn ProgressSink>,
        session_id: Option<&str>,
    ) -> Result<ChatReply> {
        if !roles::is_valid_at(role, &self.data_root) {
            return Err(AppError::InvalidInput(format!("未知角色：{role}")));
        }
        if self.current_settings().cloud.mode == "cloudOnly" {
            return Err(AppError::InvalidInput(
                "CloudOnly 模式已开启，请使用外部模型".to_string(),
            ));
        }

        let message = message.trim();
        if message.is_empty() {
            return Err(AppError::InvalidInput("消息不能为空".to_string()));
        }

        // 每轮重置工具循环守卫（决策 32）
        crate::tools::begin_turn();
        let turn_started = Instant::now();

        // 能力分层路由（决策 33）：先定档，再决定本地/云端；仅本地模式下只给"可申请云端协助"提示。
        let route = self.route_decision(message);
        tracing::info!(
            route = route.route.as_str(),
            capability = %route.capability,
            needs_cloud_consent = route.needs_cloud_consent,
            reason = %route.reason,
            "capability route decided"
        );

        let session_id = match session_id.map(str::trim).filter(|id| !id.is_empty()) {
            Some(id) => id.to_string(),
            None => self
                .session
                .latest(role)
                .map(|session| session.session_id)
                .unwrap_or_else(SessionStore::new_id),
        };

        if let Err(err) = self.memory.remember_from_user(role, message) {
            tracing::warn!(role, error = %err, "long-term memory update failed");
        }

        // 自动模式（「仅本地」关闭）：认知复杂任务直接上云、不弹窗（决策 33）。
        // 云端默认关闭，所以默认行为仍是纯本地；任何失败都优雅降级回本地执行。
        if self.current_settings().cloud.auto_escalate && route.route == Route::Cloud {
            let provider_id = self.current_settings().default_chat.clone();
            if provider_id.trim().is_empty() {
                tracing::info!("cloud enabled but no default provider set, degrading to local");
            } else {
                // 自动模式：先用本地确定性抽取生成结构化摘要，再连同问题一起上云（决策 33）
                let digest = self.cloud_digest(message);
                match self
                    .send_external_core(
                        role,
                        message,
                        Some(&session_id),
                        &provider_id,
                        false,
                        digest,
                    )
                    .await
                {
                    Ok(outcome) => {
                        if let Some(reply) = outcome.reply {
                            tracing::info!(
                                provider = %provider_id,
                                model = %reply.model,
                                capability = %route.capability,
                                "auto-escalated to cloud"
                            );
                            return Ok(ChatReply {
                                role: role.to_string(),
                                model: reply.model,
                                content: reply.content,
                                elapsed_ms: reply.elapsed_ms,
                                session_id,
                                tool_calls: Vec::new(),
                                route: Some(route),
                            });
                        }
                        tracing::warn!(
                            status = %outcome.status,
                            "cloud escalation returned no reply, degrading to local"
                        );
                    }
                    Err(err) => {
                        tracing::warn!(error = %err, "cloud escalation failed, degrading to local");
                    }
                }
            }
        }

        let history = self.session.load_session(role, &session_id);
        let model = self.config.coordinator.model.clone();
        let orchestrate = orchestrator::should_orchestrate(message);

        // 模板命中：走 Planner 代码编排（决策 31）——读 → 单步总结，模型不跨步。
        // 未命中（或执行失败）时才回落到下面的 orchestrator / coordinator 链路。
        let plan_hit = planner::auto_template(message, &orchestrator::extract_paths(message));
        let planner_reply = match plan_hit {
            Some((template, args)) => {
                match self
                    .run_planner_template(message, template, &args, prompt, &model)
                    .await
                {
                    Ok(outputs) => Some(planner_content(&outputs)),
                    Err(err) => {
                        tracing::warn!(error = %err, "planner 模板执行失败，回退到对话链路");
                        None
                    }
                }
            }
            None => None,
        };

        tracing::info!(
            role,
            model = %model,
            session_id = %session_id,
            history_messages = history.len(),
            orchestrate,
            planner = planner_reply.is_some(),
            "chat requested"
        );

        let (content, model, elapsed_ms, tool_calls) = if let Some(content) = planner_reply {
            (
                content,
                model,
                turn_started.elapsed().as_millis() as u64,
                Vec::new(),
            )
        } else if orchestrate {
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

        let sensitive_paths = tool_calls
            .iter()
            .filter(|tool| tool.ok && matches!(tool.name.as_str(), "read_file" | "grep"))
            .filter_map(|tool| tool.path.clone())
            .collect::<Vec<_>>();

        if let Err(err) = self.session.append_to(
            role,
            &session_id,
            &SessionTurn {
                user: message,
                assistant: &content,
                backend: Some("local"),
                model: Some(&model),
                sensitive_paths: &sensitive_paths,
            },
        ) {
            tracing::warn!(role, session_id = %session_id, error = %err, "persist session failed");
        }

        Ok(ChatReply {
            role: role.to_string(),
            model,
            content,
            elapsed_ms,
            session_id,
            tool_calls,
            route: Some(route),
        })
    }

    pub fn history(&self, role: &str) -> Vec<SessionMessage> {
        self.session.load(role)
    }

    pub fn history_for(&self, role: &str, session_id: Option<&str>) -> Vec<SessionMessage> {
        match session_id.map(str::trim).filter(|id| !id.is_empty()) {
            Some(id) => self.session.load_session(role, id),
            None => self.session.load(role),
        }
    }

    pub fn list_sessions(&self, role: &str) -> Vec<SessionSummary> {
        self.session.list(role)
    }

    pub fn create_session(&self, role: &str, title: Option<&str>) -> Result<SessionSummary> {
        self.session.create(role, title)
    }

    pub fn session_detail(&self, role: &str, session_id: &str) -> Result<SessionDetail> {
        self.session.load_detail(role, session_id)
    }

    pub fn rename_session(&self, role: &str, session_id: &str, title: &str) -> Result<()> {
        self.session.rename(role, session_id, title)
    }

    pub fn delete_session(&self, role: &str, session_id: &str) -> Result<()> {
        self.session.delete(role, session_id)
    }

    pub fn memory_facts(&self) -> Vec<LongTermFact> {
        self.memory.facts()
    }

    pub fn clear_history(&self, role: &str) -> Result<()> {
        self.session.clear(role)
    }

    pub fn clear_session(&self, role: &str, session_id: &str) -> Result<()> {
        self.session.clear_session(role, session_id)
    }

    pub fn settings_view(&self) -> SettingsView {
        settings::view(&self.data_root, self.current_settings())
    }

    fn current_settings(&self) -> UserSettings {
        self.settings
            .read()
            .map(|value| value.clone())
            .unwrap_or_default()
    }

    fn persist_settings(&self, next: UserSettings) -> Result<UserSettings> {
        let saved = settings::save(&self.data_root, next)?;
        let mut guard = self
            .settings
            .write()
            .map_err(|_| AppError::Config("设置状态不可用".to_string()))?;
        *guard = saved.clone();
        Ok(saved)
    }

    pub fn save_settings(&self, next: UserSettings) -> Result<SaveSettingsResult> {
        let previous = self.current_settings();
        if next.cloud.mode == "cloudOnly" {
            let usable = next
                .providers
                .iter()
                .find(|provider| provider.id == next.default_chat)
                .map(|provider| provider.enabled)
                .unwrap_or(false)
                && secrets::has(&self.data_root, &next.default_chat);
            if !usable {
                return Err(AppError::InvalidInput(
                    "CloudOnly 模式需要选择已启用且配置了 API Key 的 Provider".to_string(),
                ));
            }
        } else if next.default_chat != "local"
            && !next
                .providers
                .iter()
                .any(|provider| provider.id == next.default_chat)
        {
            return Err(AppError::InvalidInput("默认对话模型不存在".to_string()));
        }
        let saved = self.persist_settings(next)?;
        self.apply_cloud_preferences(&saved);
        let restart_required = previous.models.ollama_host != saved.models.ollama_host
            || previous.models.router != saved.models.router
            || previous.models.coordinator != saved.models.coordinator
            || previous.data.root != saved.data.root;
        Ok(SaveSettingsResult {
            view: settings::view(&self.data_root, saved),
            restart_required,
        })
    }

    /// 自动获取 Provider 模型列表；未传 key 时回退到已保存的密钥。
    pub async fn fetch_provider_models(
        &self,
        base_url: &str,
        api_key: Option<&str>,
        provider_id: Option<&str>,
    ) -> Result<Vec<String>> {
        let key = match api_key.map(str::trim).filter(|value| !value.is_empty()) {
            Some(value) => value.to_string(),
            None => provider_id
                .and_then(|id| secrets::get(&self.data_root, id))
                .unwrap_or_default(),
        };
        self.providers.list_models(base_url, &key).await
    }

    /// 解析模型目录（支持 `{程序目录}` / `{app_root}` 占位符）
    pub fn models_dir(&self) -> PathBuf {
        let raw = self.models.models_dir.trim();
        if raw.is_empty() {
            return self.app_root.join("models");
        }
        let root = self.app_root.display().to_string();
        let resolved = raw
            .replace("{程序目录}", &root)
            .replace("{app_root}", &root);
        PathBuf::from(resolved)
    }

    /// 扫描 models 目录里的 GGUF 候选（含推断的名称/参数量/量化/档位）
    pub async fn scan_models(&self) -> Result<Vec<crate::model_import::GgufCandidate>> {
        let dir = self.models_dir();
        let installed = self
            .ollama
            .list_models(Duration::from_millis(1500))
            .await
            .unwrap_or_default();
        Ok(crate::model_import::scan(&dir, &installed))
    }

    /// 导入一个 GGUF：生成 Modelfile → ollama create → 登记 models.json
    pub async fn import_gguf(
        &self,
        request: crate::model_import::ImportRequest,
    ) -> Result<crate::model_import::ImportResult> {
        let dir = self.models_dir();
        let mut result = crate::model_import::import(&self.app_root, &dir, &request)?;

        // 档位空缺时自动选用新模型（只改选择，不影响已有配置）
        let installed = self
            .ollama
            .list_models(Duration::from_millis(1500))
            .await
            .unwrap_or_default();
        let mut settings = self.current_settings();
        let mut changed = false;
        if result.grade == "light"
            && !installed
                .iter()
                .any(|item| same_model(item, &settings.models.router))
        {
            settings.models.router = result.model_name.clone();
            changed = true;
        }
        if result.grade == "heavy"
            && !installed
                .iter()
                .any(|item| same_model(item, &settings.models.coordinator))
        {
            settings.models.coordinator = result.model_name.clone();
            changed = true;
        }
        if changed {
            if let Err(err) = self.persist_settings(settings) {
                tracing::warn!(error = %err, "模型档位写回设置失败");
            } else {
                result.switched_selection = true;
            }
        }
        Ok(result)
    }

    /// 打开本地目录（白名单：models / data / logs）
    pub fn open_local_path(&self, kind: &str) -> Result<PathBuf> {
        let path = match kind.trim() {
            "models" => self.models_dir(),
            "data" => self.data_root.clone(),
            "logs" => self.data_root.join("logs"),
            other => {
                return Err(AppError::InvalidInput(format!(
                    "不支持的目录类型：{other}（可选 models / data / logs）"
                )))
            }
        };
        std::fs::create_dir_all(&path)?;
        #[cfg(windows)]
        {
            std::process::Command::new("explorer")
                .arg(&path)
                .spawn()
                .map_err(|err| {
                    AppError::Io(std::io::Error::new(
                        err.kind(),
                        format!("打开目录失败：{err}"),
                    ))
                })?;
        }
        Ok(path)
    }

    pub fn providers_view(&self) -> Vec<ProviderView> {
        let settings = self.current_settings();
        let cache = self.provider_health.lock().ok();
        settings
            .providers
            .iter()
            .map(|provider| {
                let reachable = cache
                    .as_ref()
                    .and_then(|cache| cache.get(&provider.id))
                    .filter(|entry| entry.fresh())
                    .map(|entry| entry.reachable);
                providers::view(
                    provider,
                    secrets::has(&self.data_root, &provider.id),
                    reachable,
                )
            })
            .collect()
    }

    pub fn save_provider(&self, input: ProviderInput) -> Result<ProviderView> {
        let api_key = input.api_key.clone();
        let provider = providers::normalize(input)?;
        let mut settings = self.current_settings();
        if let Some(existing) = settings
            .providers
            .iter_mut()
            .find(|existing| existing.id == provider.id)
        {
            *existing = provider.clone();
        } else {
            settings.providers.push(provider.clone());
        }
        if !provider.enabled && settings.default_chat == provider.id {
            settings.default_chat = "local".to_string();
            if settings.cloud.mode == "cloudOnly" {
                settings.cloud.mode = "localFirst".to_string();
            }
        }
        let saved = self.persist_settings(settings)?;
        if let Some(key) = api_key.as_deref() {
            secrets::set(&self.data_root, &provider.id, Some(key))?;
        }
        let stored = saved
            .providers
            .iter()
            .find(|item| item.id == provider.id)
            .cloned()
            .unwrap_or(provider);
        Ok(providers::view(
            &stored,
            secrets::has(&self.data_root, &stored.id),
            self.cached_health(&stored.id),
        ))
    }

    pub fn delete_provider(&self, id: &str) -> Result<()> {
        let mut settings = self.current_settings();
        let before = settings.providers.len();
        settings.providers.retain(|provider| provider.id != id);
        if settings.providers.len() == before {
            return Err(AppError::InvalidInput(format!("未知 Provider：{id}")));
        }
        settings
            .file_export_grants
            .retain(|grant| grant.provider_id != id);
        if settings.default_chat == id {
            settings.default_chat = "local".to_string();
            if settings.cloud.mode == "cloudOnly" {
                settings.cloud.mode = "localFirst".to_string();
            }
        }
        self.persist_settings(settings)?;
        secrets::remove(&self.data_root, id)?;
        if let Ok(mut cache) = self.provider_health.lock() {
            cache.remove(id);
        }
        Ok(())
    }

    pub async fn test_provider(&self, id: &str, force: bool) -> Result<ProviderTest> {
        let provider = self.find_provider(id)?;
        if !force {
            if let Some(reachable) = self.cached_health(id) {
                return Ok(ProviderTest {
                    id: id.to_string(),
                    reachable,
                    cached: true,
                    error: None,
                });
            }
        }
        let key = secrets::get(&self.data_root, id)
            .ok_or_else(|| AppError::InvalidInput("Provider 未配置 API Key".to_string()))?;
        let result = self.providers.test(&provider, &key).await;
        let (reachable, error) = match result {
            Ok(()) => (true, None),
            Err(err) => (false, Some(err.to_string())),
        };
        if let Ok(mut cache) = self.provider_health.lock() {
            cache.insert(
                id.to_string(),
                HealthEntry {
                    checked_at: Instant::now(),
                    reachable,
                },
            );
        }
        Ok(ProviderTest {
            id: id.to_string(),
            reachable,
            cached: false,
            error,
        })
    }

    pub fn prepare_external(
        &self,
        role: &str,
        session_id: Option<&str>,
        provider_id: &str,
    ) -> Result<ExportGate> {
        if !roles::is_valid_at(role, &self.data_root) {
            return Err(AppError::InvalidInput(format!("未知角色：{role}")));
        }
        let provider = self.find_provider(provider_id)?;
        if !provider.enabled {
            return Err(AppError::InvalidInput("Provider 未启用".to_string()));
        }
        if !secrets::has(&self.data_root, provider_id) {
            return Err(AppError::InvalidInput(
                "Provider 未配置 API Key".to_string(),
            ));
        }
        let paths = self.last_sensitive_paths(role, session_id);
        Ok(ExportGate {
            role_id: role.to_string(),
            provider_id: provider.id,
            granted: self.has_export_grant(role, provider_id),
            paths,
        })
    }

    pub fn grant_file_export(&self, role_id: &str, provider_id: &str) -> Result<()> {
        if !roles::is_valid_at(role_id, &self.data_root) {
            return Err(AppError::InvalidInput(format!("未知角色：{role_id}")));
        }
        self.find_provider(provider_id)?;
        let mut settings = self.current_settings();
        if !settings
            .file_export_grants
            .iter()
            .any(|grant| grant.role_id == role_id && grant.provider_id == provider_id)
        {
            settings.file_export_grants.push(settings::FileExportGrant {
                role_id: role_id.to_string(),
                provider_id: provider_id.to_string(),
            });
        }
        self.persist_settings(settings)?;
        Ok(())
    }

    pub fn revoke_file_export(&self, provider_id: &str) -> Result<()> {
        let mut settings = self.current_settings();
        settings
            .file_export_grants
            .retain(|grant| grant.provider_id != provider_id);
        self.persist_settings(settings)?;
        Ok(())
    }

    pub async fn send_external_message(
        &self,
        role: &str,
        message: &str,
        session_id: Option<&str>,
        provider_id: &str,
        include_context: bool,
    ) -> Result<ExternalChatOutcome> {
        self.send_external_core(
            role,
            message,
            session_id,
            provider_id,
            include_context,
            None,
        )
        .await
    }

    /// 外发核心：`extra_context` 为上云前由本地确定性抽取的结构化摘要（决策 33）。
    async fn send_external_core(
        &self,
        role: &str,
        message: &str,
        session_id: Option<&str>,
        provider_id: &str,
        include_context: bool,
        extra_context: Option<String>,
    ) -> Result<ExternalChatOutcome> {
        if !roles::is_valid_at(role, &self.data_root) {
            return Err(AppError::InvalidInput(format!("未知角色：{role}")));
        }
        let message = message.trim();
        if message.is_empty() {
            return Err(AppError::InvalidInput("消息不能为空".to_string()));
        }

        let provider = self.find_provider(provider_id)?;
        if !provider.enabled {
            return Err(AppError::InvalidInput("Provider 未启用".to_string()));
        }
        let key = secrets::get(&self.data_root, provider_id)
            .ok_or_else(|| AppError::InvalidInput("Provider 未配置 API Key".to_string()))?;
        let model = if provider.default_model.trim().is_empty() {
            provider.models.first().cloned().unwrap_or_default()
        } else {
            provider.default_model.clone()
        };
        if model.is_empty() {
            return Err(AppError::InvalidInput(
                "Provider 未配置默认模型".to_string(),
            ));
        }

        let session_id = match session_id.map(str::trim).filter(|id| !id.is_empty()) {
            Some(id) => id.to_string(),
            None => match self.session.latest(role) {
                Some(session) => session.session_id,
                None => self.session.create(role, None)?.session_id,
            },
        };
        let messages = self
            .session
            .load_detail(role, &session_id)
            .map(|detail| detail.messages)
            .unwrap_or_default();
        let sensitive_paths = if include_context {
            self.last_sensitive_paths(role, Some(&session_id))
        } else {
            Vec::new()
        };
        if !sensitive_paths.is_empty() && !self.has_export_grant(role, provider_id) {
            return Ok(ExternalChatOutcome::export_required(ExportGate {
                role_id: role.to_string(),
                provider_id: provider.id,
                paths: sensitive_paths,
                granted: false,
            }));
        }

        let mut system = roles::system_prompt(role, &self.app_root, &self.data_root);
        let memory = self.memory.context();
        if include_context && !memory.trim().is_empty() {
            system.push_str("\n\n");
            system.push_str(&memory);
        }
        system.push_str(
            "\n\n当前为外部模型模式：请直接回答，不要假设可以调用本地工具或读取未提供的文件。",
        );

        let mut request_messages = vec![("system".to_string(), system)];
        if include_context {
            let start = messages.len().saturating_sub(12);
            for entry in &messages[start..] {
                if entry.role == "user" || entry.role == "assistant" {
                    request_messages.push((entry.role.clone(), entry.content.clone()));
                }
            }
            if !sensitive_paths.is_empty() {
                let files = providers::read_export_files(&sensitive_paths);
                if !files.is_empty() {
                    let mut context =
                        String::from("以下是用户已授权导出的本地文件内容，仅用于本轮回答：\n");
                    for (path, content) in files {
                        context.push_str(&format!("\n--- {path} ---\n{content}\n"));
                    }
                    request_messages.push(("system".to_string(), context));
                }
            }
        }
        // 上云前的本地确定性抽取结果（决策 33）：目录树 / 符号表 / 命中行，不是指令。
        if let Some(extra) = extra_context {
            if !extra.trim().is_empty() {
                request_messages.push(("system".to_string(), extra));
            }
        }
        request_messages.push(("user".to_string(), message.to_string()));

        if let Err(err) = self.memory.remember_from_user(role, message) {
            tracing::warn!(role, error = %err, "long-term memory update failed");
        }

        // 隐私红线 + 会话预算（决策 33）：红线不随「仅本地」开关变化，命中即拒发。
        let joined: String = request_messages
            .iter()
            .map(|(_, content)| content.as_str())
            .collect::<Vec<_>>()
            .join("\n");
        let red_line_sources: Vec<String> = sensitive_paths
            .iter()
            .filter(|path| crate::egress::is_sensitive_path(Path::new(path.as_str())))
            .cloned()
            .collect();
        let report = crate::egress::inspect(&joined, &red_line_sources);
        if let Some(reason) = report.blocked {
            tracing::warn!(provider = %provider_id, reason = %reason, "cloud egress blocked");
            audit::event(
                &self.data_root,
                "cloud_egress",
                role,
                "blocked",
                "denied",
                serde_json::json!({
                    "provider": provider_id,
                    "reason": reason,
                    "findings": report.findings.iter().map(|item| item.kind.clone()).collect::<Vec<_>>(),
                    "estimated_tokens": report.estimated_tokens
                }),
            );
            return Err(AppError::InvalidInput(reason));
        }
        {
            let mut budget = self
                .cloud_budget
                .lock()
                .map_err(|_| AppError::InvalidInput("云端预算状态不可用".to_string()))?;
            if !budget.try_consume(report.estimated_tokens) {
                let used = budget.used_tokens;
                let limit = budget.limit_tokens.unwrap_or(0);
                tracing::warn!(provider = %provider_id, used, limit, "cloud budget exhausted");
                return Err(AppError::InvalidInput(format!(
                    "本次会话云端预算已用尽（已用 {used} / 上限 {limit} tokens），已降级为仅本地；如需继续请在设置里提高上限"
                )));
            }
        }

        let mut reply = self
            .providers
            .chat(&provider, &key, &model, &request_messages)
            .await?;
        reply.role = role.to_string();

        audit::event(
            &self.data_root,
            "cloud_egress",
            role,
            "sent",
            "success",
            serde_json::json!({
                "provider": provider_id,
                "model": reply.model,
                "estimated_tokens": report.estimated_tokens,
                "include_context": include_context,
                "granted_paths": sensitive_paths.len()
            }),
        );

        if let Err(err) = self.session.append_to(
            role,
            &session_id,
            &SessionTurn {
                user: message,
                assistant: &reply.content,
                backend: Some("external"),
                model: Some(&reply.model),
                sensitive_paths: &sensitive_paths,
            },
        ) {
            tracing::warn!(
                role,
                session_id = %session_id,
                error = %err,
                "persist external session failed"
            );
        }

        tracing::info!(
            role,
            provider = %provider_id,
            model = %reply.model,
            session_id = %session_id,
            "external chat complete"
        );
        Ok(ExternalChatOutcome::ok(reply))
    }

    fn find_provider(&self, id: &str) -> Result<ProviderSettings> {
        self.current_settings()
            .providers
            .into_iter()
            .find(|provider| provider.id == id)
            .ok_or_else(|| AppError::InvalidInput(format!("未知 Provider：{id}")))
    }

    fn cached_health(&self, id: &str) -> Option<bool> {
        let cache = self.provider_health.lock().ok()?;
        cache
            .get(id)
            .filter(|entry| entry.fresh())
            .map(|entry| entry.reachable)
    }

    fn has_export_grant(&self, role_id: &str, provider_id: &str) -> bool {
        self.current_settings()
            .file_export_grants
            .iter()
            .any(|grant| grant.role_id == role_id && grant.provider_id == provider_id)
    }

    fn last_sensitive_paths(&self, role: &str, session_id: Option<&str>) -> Vec<String> {
        let messages = match session_id.map(str::trim).filter(|id| !id.is_empty()) {
            Some(id) => self
                .session
                .load_detail(role, id)
                .map(|detail| detail.messages)
                .unwrap_or_default(),
            None => self
                .session
                .latest(role)
                .and_then(|session| self.session.load_detail(role, &session.session_id).ok())
                .map(|detail| detail.messages)
                .unwrap_or_default(),
        };
        sensitive_paths_from(&messages)
    }

    pub fn roles(&self) -> Vec<RoleSummary> {
        roles::list(&self.data_root)
    }

    pub fn save_role(&self, role: RoleSummary) -> Result<RoleSummary> {
        roles::save_custom(&self.data_root, role)
    }

    pub fn delete_role(&self, id: &str, delete_data: bool) -> Result<()> {
        roles::delete_custom(&self.data_root, id, delete_data)
    }

    pub async fn list_local_models(&self) -> Result<Vec<String>> {
        self.ollama.list_models(Duration::from_millis(1500)).await
    }
}

fn sensitive_paths_from(messages: &[SessionMessage]) -> Vec<String> {
    for message in messages.iter().rev() {
        if message.role == "assistant" && message.sensitive && !message.sensitive_paths.is_empty() {
            let mut paths = Vec::new();
            for path in &message.sensitive_paths {
                if !paths.contains(path) {
                    paths.push(path.clone());
                }
            }
            return paths;
        }
    }
    Vec::new()
}
