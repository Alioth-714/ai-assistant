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
use crate::ollama::{ChatMessage, ChatOptions, OllamaClient};
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
use crate::settings::{
    self, ModelSetting, ProviderSettings, SaveSettingsResult, SettingsView, UserSettings,
};
use serde::{Deserialize, Serialize};
use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, RwLock};
use std::time::{Duration, Instant};
use tracing_appender::non_blocking::WorkerGuard;

/// 云端调用的滑动窗口：时长与阈值（按次数计）
pub const CLOUD_WINDOW: Duration = Duration::from_secs(60 * 60);
pub const CLOUD_WINDOW_LIMIT: usize = 20;

/// 会话云端用量快照
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudUsage {
    pub session_id: String,
    pub used: usize,
    pub limit: usize,
    pub window_minutes: u32,
    pub blocked: bool,
    /// 用户是否已选择"本次运行不再询问"
    pub suppressed: bool,
}

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
    /// 会话云端调用次数（滑动窗口；本次运行内有效）
    cloud_usage: Mutex<HashMap<String, VecDeque<Instant>>>,
    /// 用户在弹窗里选过"本次运行不再询问"
    cloud_prompt_suppressed: Mutex<bool>,
    /// 已切换但尚未重启生效的新数据目录（用于「打开目录」与界面提示）
    pending_data_root: RwLock<Option<PathBuf>>,
    /// 本次会话里登记过的所有切换目标（撤销时一次性清掉它们的标记）
    pending_switches: RwLock<Vec<PathBuf>>,
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

/// 设置页模型下拉的选项（决策 33：按档位过滤 + 排除 embedding）
#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct ModelOption {
    pub name: String,
    /// light / heavy / embedding / unknown
    pub grade: String,
    /// chat / embedding
    pub kind: String,
    /// 是否已在 config/models.json 登记（未登记的模型不出现在下拉里）
    pub registered: bool,
}

/// 名字看起来像 embedding 模型
fn looks_like_embedding(name: &str) -> bool {
    let lower = name.to_lowercase();
    lower.contains("embed")
        || lower.contains("bge")
        || lower.contains("gte-")
        || lower.contains("e5-")
        || lower.contains("reranker")
}

/// 从名字猜档位（仅用于未登记在 models.json 的模型）
fn guess_grade(name: &str) -> String {
    let lower = name.to_lowercase();
    for token in lower.split(['-', '_', ':']) {
        let Some(rest) = token.strip_suffix('b') else {
            continue;
        };
        if let Ok(value) = rest.parse::<f64>() {
            if value > 0.0 {
                return if value <= 4.0 { "light" } else { "heavy" }.to_string();
            }
        }
    }
    "unknown".to_string()
}

/// 去掉 Windows `canonicalize` 产生的 `\\?\` 扩展前缀
fn strip_extended_prefix(path: PathBuf) -> PathBuf {
    let text = path.display().to_string();
    if let Some(rest) = text.strip_prefix(r"\\?\") {
        return PathBuf::from(rest);
    }
    path
}

/// 把 `models.json` 里的 `models_dir` 占位符替换成真实路径
fn resolve_models_dir(raw: &str, app_root: &Path) -> PathBuf {
    let raw = raw.trim();
    if raw.is_empty() {
        return app_root.join("models");
    }
    let root = app_root.display().to_string();
    PathBuf::from(
        raw.replace("{程序目录}", &root)
            .replace("{app_root}", &root),
    )
}

/// 用 ShellExecuteW 打开目录（比 explorer.exe 传参可靠：不会静默退回「文档」）
#[cfg(windows)]
fn open_directory(path: &Path) -> Result<()> {
    use std::ffi::c_void;
    use std::os::windows::ffi::OsStrExt;
    use std::ptr;

    #[link(name = "shell32")]
    extern "system" {
        fn ShellExecuteW(
            hwnd: *mut c_void,
            lp_operation: *const u16,
            lp_file: *const u16,
            lp_parameters: *const u16,
            lp_directory: *const u16,
            n_show_cmd: i32,
        ) -> *mut c_void;
    }

    // 目录不存在时 ShellExecute 会打开默认位置，所以先校验
    if !path.is_dir() {
        return Err(AppError::InvalidInput(format!(
            "目录不存在：{}",
            path.display()
        )));
    }

    let operation: Vec<u16> = "open".encode_utf16().chain(Some(0)).collect();
    let file: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
    let result = unsafe {
        ShellExecuteW(
            ptr::null_mut(),
            operation.as_ptr(),
            file.as_ptr(),
            ptr::null(),
            ptr::null(),
            1, // SW_SHOWNORMAL
        )
    };
    if (result as isize) <= 32 {
        return Err(AppError::Io(std::io::Error::other(format!(
            "打开目录失败（ShellExecute 返回 {}）：{}",
            result as isize,
            path.display()
        ))));
    }
    tracing::info!(path = %path.display(), "opened directory");
    Ok(())
}

#[cfg(not(windows))]
fn open_directory(_path: &Path) -> Result<()> {
    Ok(())
}

/// 生成角色提示词的元提示词
const GENERATE_ROLE_PROMPT_SYSTEM: &str = r#"你是角色提示词设计师。根据给定的角色名称与简介，产出一份专业的系统提示词。
只输出 JSON，不要 Markdown，不要解释。格式：
{"systemPrompt":"...","guardrails":["..."],"outputStyle":"concise|structured"}

systemPrompt 要求：
1. 第一句写「你是「角色名」，……」明确身份与边界；
2. 必须有「工作方式」，4-6 条可执行原则；
3. 必须有「输出要求」，说明结构、长度与先给结论；
4. 明确「不确定时怎么做」，禁止编造；
5. 若角色涉及医疗、心理、法律、投资等高风险领域，必须写「不做诊断/不作结论/建议咨询专业人士」等边界，并提示紧急情况如何处理；
6. 用中文，不要出现「作为一个 AI 模型」这类空话。

guardrails 是 3-6 条机器可读的硬性禁止项，每条一句话。
outputStyle：偏咨询/评估类用 structured，其余用 concise。"#;

fn generate_role_prompt_schema() -> serde_json::Value {
    serde_json::json!({
        "type": "object",
        "properties": {
            "systemPrompt": { "type": "string" },
            "guardrails": { "type": "array", "items": { "type": "string" } },
            "outputStyle": { "type": "string", "enum": ["concise", "structured"] }
        },
        "required": ["systemPrompt", "guardrails", "outputStyle"]
    })
}

/// 比较两个目录是否等价（按规范化路径，忽略大小写）
fn same_dir(left: &Path, right: &Path) -> bool {
    let normalize = |path: &Path| {
        strip_extended_prefix(path.canonicalize().unwrap_or_else(|_| path.to_path_buf()))
            .display()
            .to_string()
            .to_lowercase()
    };
    normalize(left) == normalize(right)
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
    std::fs::create_dir_all(&data_root)?;
    // 上次切换登记的"搬数据"意图：此处程序尚未打开任何数据文件，是最安全的执行时机
    if let Some(old_root) = crate::data::take_pending_move(&data_root) {
        match crate::data::migrate_entries(&old_root, &data_root) {
            Ok((files, bytes)) => tracing::info!(
                from = %old_root.display(),
                files,
                bytes,
                "pending move completed"
            ),
            Err(err) => tracing::warn!(
                from = %old_root.display(),
                error = %err,
                "pending move failed, old directory kept"
            ),
        }
    }
    crate::data::ensure_layout(&data_root)?;
    // 旧版本遗留的清理标记
    if let Some(removed) = crate::data::run_pending_cleanup(&data_root) {
        tracing::info!(path = %removed.display(), "pending cleanup done");
    }

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
        data_dir_source = crate::data::root_source(),
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
        cloud_usage: Mutex::new(HashMap::new()),
        cloud_prompt_suppressed: Mutex::new(false),
        pending_data_root: RwLock::new(None),
        pending_switches: RwLock::new(Vec::new()),
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
        self.route_decision_with(message, None)
    }

    /// 按角色策略定档：
    /// - `localOnly`（large profile 下的健康/心理）：即使开关关闭也不上云
    /// - `localFirst`（small profile 下的健康/心理）：本地优先，但需用户显式放权才上云
    /// - `default`：按全局开关与能力白名单决定
    pub fn route_decision_for_role(&self, role: &str, message: &str) -> RouteDecision {
        match self.role_cloud_policy(role).as_str() {
            "localOnly" => self.route_decision_with(message, Some(true)),
            "localFirst" => {
                let mut decision = self.route_decision_with(message, Some(true));
                // 本地优先：默认本地，但明确告知"可申请云端分析"
                decision.route = capabilities::Route::LocalWithCloudHint;
                decision.needs_cloud_consent = true;
                decision.reason = format!("{}；该角色本地优先，可显式放权上云", decision.reason);
                decision
            }
            _ => self.route_decision_with(message, None),
        }
    }

    /// 当前 profile 下该角色的云端策略（default / localOnly / localFirst）
    fn role_cloud_policy(&self, role: &str) -> String {
        let profile = self
            .capabilities
            .read()
            .map(|guard| guard.profile.clone())
            .unwrap_or_else(|_| "small".to_string());
        roles::load_role(&self.app_root, &self.data_root, role)
            .map(|item| item.cloud_policy.resolve(&profile))
            .unwrap_or_else(|| "default".to_string())
    }

    fn route_decision_with(&self, message: &str, force_local_only: Option<bool>) -> RouteDecision {
        let local_only = force_local_only.unwrap_or(!self.current_settings().cloud.auto_escalate);
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

    /// 应用云端偏好（决策 33）：能力档位，保存设置后即时生效、无需重启。
    fn apply_cloud_preferences(&self, settings: &UserSettings) {
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

    /// 记录一次云端调用（按会话滑动窗口统计）
    pub fn record_cloud_call(&self, session_id: &str) {
        let now = Instant::now();
        if let Ok(mut usage) = self.cloud_usage.lock() {
            let entry = usage.entry(session_id.to_string()).or_default();
            entry.retain(|at| now.duration_since(*at) < CLOUD_WINDOW);
            entry.push_back(now);
            tracing::info!(
                session = %session_id,
                used = entry.len(),
                limit = CLOUD_WINDOW_LIMIT,
                "cloud call recorded"
            );
        }
    }

    /// 该会话是否已被窗口阈值阻断（且用户没有选择"本次运行不再询问"）
    pub fn cloud_allowed(&self, session_id: &str) -> bool {
        if self
            .cloud_prompt_suppressed
            .lock()
            .map(|flag| *flag)
            .unwrap_or(false)
        {
            return true;
        }
        !self.cloud_usage_snapshot(session_id).blocked
    }

    /// 云端用量快照（供前端提示与阻断判断）
    pub fn cloud_usage(&self, session_id: &str) -> CloudUsage {
        self.cloud_usage_snapshot(session_id)
    }

    fn cloud_usage_snapshot(&self, session_id: &str) -> CloudUsage {
        let now = Instant::now();
        let used = self
            .cloud_usage
            .lock()
            .ok()
            .map(|mut usage| {
                let entry = usage.entry(session_id.to_string()).or_default();
                entry.retain(|at| now.duration_since(*at) < CLOUD_WINDOW);
                entry.len()
            })
            .unwrap_or(0);
        CloudUsage {
            session_id: session_id.to_string(),
            used,
            limit: CLOUD_WINDOW_LIMIT,
            window_minutes: (CLOUD_WINDOW.as_secs() / 60) as u32,
            blocked: used >= CLOUD_WINDOW_LIMIT,
            suppressed: self
                .cloud_prompt_suppressed
                .lock()
                .map(|flag| *flag)
                .unwrap_or(false),
        }
    }

    /// 本次运行内"不再询问"
    pub fn set_cloud_prompt_suppressed(&self, value: bool) {
        if let Ok(mut flag) = self.cloud_prompt_suppressed.lock() {
            *flag = value;
        }
        tracing::info!(suppressed = value, "cloud prompt suppression changed");
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

    /// 切换数据目录。
    /// - `migrate = true`：复制现有数据并逐字节校验，成功后标记"重启后删除旧目录"
    /// - `migrate = false`：直接使用空的新目录（旧目录保持不动）
    ///
    /// 目标目录必须是空目录；失败时指针不变、旧目录不受影响。重启后生效。
    pub fn set_data_root(
        &self,
        target: &str,
        migrate: bool,
    ) -> Result<crate::data::MigrationReport> {
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
        if same_dir(&current, &target_path) {
            return Err(AppError::InvalidInput(
                "目标目录与当前数据目录相同，无需切换".to_string(),
            ));
        }
        if target_path.starts_with(&current) || current.starts_with(&target_path) {
            return Err(AppError::InvalidInput(
                "目标目录不能与当前数据目录互相嵌套".to_string(),
            ));
        }
        // 只允许空目录，避免半合并状态
        if target_path.exists() {
            let empty = std::fs::read_dir(&target_path)
                .map(|mut entries| entries.next().is_none())
                .unwrap_or(false);
            if !empty {
                return Err(AppError::InvalidInput(
                    "目标目录不是空目录，请选择空文件夹或新建一个".to_string(),
                ));
            }
        }
        std::fs::create_dir_all(&target_path)?;
        crate::data::ensure_layout(&target_path)?;

        // 切换时立即复制并逐字节校验：复制没完成就不能进入"关闭倒计时"，
        // 否则大文件还没搬完程序就被关掉了。
        let report = if migrate {
            let report = crate::data::migrate(&current, &target_path)?;
            if !report.verified {
                return Err(AppError::InvalidInput(
                    "迁移后校验未通过，指针未改动；旧目录不受影响".to_string(),
                ));
            }
            report
        } else {
            crate::data::MigrationReport {
                source: current.display().to_string(),
                target: target_path.display().to_string(),
                files: 0,
                bytes: 0,
                verified: true,
                skipped: 0,
            }
        };

        crate::data::write_pointer(&target_path)?;
        if let Ok(mut guard) = self.pending_data_root.write() {
            *guard = Some(target_path.clone());
        }
        if let Ok(mut list) = self.pending_switches.write() {
            if !list.contains(&target_path) {
                list.push(target_path.clone());
            }
        }
        if migrate {
            // 运行期不删旧目录：等重启后由 bootstrap 清理，避免正在写入的组件失败
            if let Err(err) = crate::data::mark_pending_cleanup(&target_path, &current) {
                tracing::warn!(error = %err, "写入待清理标记失败，旧目录需手工删除");
            }
        }
        audit::event(
            &self.data_root,
            "data_root",
            "user",
            if migrate { "migrate" } else { "switch" },
            "success",
            serde_json::json!({
                "target": target_path.display().to_string(),
                "migrate": migrate,
                "files": report.files,
                "bytes": report.bytes,
                "skipped": report.skipped
            }),
        );
        tracing::info!(
            target = %target_path.display(),
            migrate,
            files = report.files,
            bytes = report.bytes,
            "data root switched, restart required"
        );
        Ok(report)
    }

    /// 会话云端用量（次数 / 窗口）
    pub fn cloud_usage_of(&self, session_id: &str) -> CloudUsage {
        self.cloud_usage_snapshot(session_id)
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

    /// 路由与协调是否指向同一个本地模型（单模型模式）
    pub fn single_local_model(&self) -> bool {
        same_model(&self.config.router.model, &self.config.coordinator.model)
    }

    /// 单模型模式下统一使用的上下文长度
    pub fn active_num_ctx(&self) -> u32 {
        self.config.coordinator.max_tokens.max(2048)
    }

    pub async fn route(&self, content: &str) -> RouterDecision {
        // 单模型模式必须与协调用同一个 num_ctx，否则 Ollama 会反复重载模型
        let num_ctx = if self.single_local_model() {
            self.active_num_ctx()
        } else {
            crate::router::ROUTER_NUM_CTX
        };
        crate::router::route(&self.ollama, &self.config, content, num_ctx).await
    }

    pub async fn warmup_router(&self) {
        if self.single_local_model() {
            tracing::info!("router warmup skipped: same model as coordinator");
            return;
        }
        let model = &self.config.router.model;
        match self
            .ollama
            .preload(
                model,
                crate::router::ROUTER_NUM_CTX,
                Duration::from_secs(60),
            )
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
        let route = self.route_decision_for_role(role, message);
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

        if self.should_write_memory(role, message) {
            if let Err(err) = self.memory.remember_from_user(role, message) {
                tracing::warn!(role, error = %err, "long-term memory update failed");
            }
        }

        // 自动模式（「仅本地」关闭）：认知复杂任务直接上云、不弹窗（决策 33）。
        // 云端默认关闭，所以默认行为仍是纯本地；任何失败都优雅降级回本地执行。
        // 自动升级只对 default 策略的角色生效；健康/心理（localFirst）必须显式放权；
        // 同一会话超过频次阈值后也不再自动上云，改为提示用户确认。
        if self.current_settings().cloud.auto_escalate
            && route.route == Route::Cloud
            && self.role_cloud_policy(role) == "default"
        {
            if !self.cloud_allowed(&session_id) {
                tracing::info!(session = %session_id, "auto escalate skipped: window limit reached");
            } else {
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

        // 本地用量记账：tokens 按字符数粗估（标"约"），费用恒为 0
        {
            let now_ms = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|value| value.as_millis() as u64)
                .unwrap_or(0);
            let estimate_in = (message.chars().count() as u64).div_ceil(2);
            let estimate_out = (content.chars().count() as u64).div_ceil(2);
            let entry = crate::usage::UsageEntry {
                ts: now_ms,
                provider_id: "local".to_string(),
                model_id: model.clone(),
                tokens_in: estimate_in,
                tokens_out: estimate_out,
                cache_tokens: 0,
                cost: 0.0,
            };
            if let Err(err) = crate::usage::record(&self.data_root, &entry) {
                tracing::warn!(error = %err, "本地用量记账失败");
            }
        }

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
            && !next
                .external_models
                .iter()
                .any(|item| item.id == next.default_chat)
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

    /// 设置页模型选项：读最新 models.json 的档位 + ollama 已安装列表，并排除 embedding
    pub async fn model_options(&self) -> Vec<ModelOption> {
        let installed = self
            .ollama
            .list_models(Duration::from_millis(1500))
            .await
            .unwrap_or_default();
        let models_cfg = crate::config::load(&self.app_root)
            .map(|(_, models)| models)
            .unwrap_or_else(|_| self.models.clone());

        let mut options: Vec<ModelOption> = installed
            .into_iter()
            .map(|name| {
                let key = name
                    .trim()
                    .trim_end_matches(":latest")
                    .trim_end_matches(':')
                    .to_string();
                let entry = models_cfg.installed.get(&key);
                let registered = entry.is_some();
                let grade = entry
                    .map(|item| item.grade.clone())
                    .filter(|grade| !grade.trim().is_empty())
                    .unwrap_or_else(|| guess_grade(&key));
                let kind = if grade == "embedding" || looks_like_embedding(&key) {
                    "embedding"
                } else {
                    "chat"
                };
                ModelOption {
                    name: key,
                    grade,
                    kind: kind.to_string(),
                    registered,
                }
            })
            .collect();
        options.sort_by(|left, right| left.name.cmp(&right.name));
        options.dedup_by(|left, right| left.name == right.name);
        options
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
        let mut models = self.providers.list_models(base_url, &key).await?;
        // 去重并排序：不同网关可能重复返回同一 id
        models.sort();
        models.dedup();
        tracing::info!(
            count = models.len(),
            sample = ?models.iter().take(8).collect::<Vec<_>>(),
            "provider models fetched"
        );
        audit::event(
            &self.data_root,
            "provider_models",
            provider_id.unwrap_or("custom"),
            "fetch",
            "success",
            serde_json::json!({
                "baseUrl": base_url,
                "count": models.len(),
                "sample": models.iter().take(20).collect::<Vec<_>>()
            }),
        );
        Ok(models)
    }

    /// 解析模型目录（支持 `{程序目录}` / `{app_root}` 占位符）
    pub fn models_dir(&self) -> PathBuf {
        resolve_models_dir(&self.models.models_dir, &self.app_root)
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

    /// 撤销尚未生效的目录切换：指针写回当前生效目录，清掉所有目标目录里的迁移标记
    pub fn abort_data_switch(&self) -> Result<PathBuf> {
        let mut targets: Vec<PathBuf> = self
            .pending_switches
            .read()
            .map(|guard| guard.clone())
            .unwrap_or_default();
        if targets.is_empty() {
            if let Some(pending) = self
                .pending_data_root
                .read()
                .ok()
                .and_then(|guard| guard.clone())
            {
                targets.push(pending);
            }
        }
        if targets.is_empty() {
            return Err(AppError::InvalidInput("当前没有待生效的切换".to_string()));
        }

        crate::data::write_pointer(&self.data_root)?;
        crate::data::clear_pending_markers(&targets);
        // 撤销时把刚复制过去的内容清掉（源数据完好），避免留下多余的整份副本
        for target in &targets {
            if same_dir(target, &self.data_root) {
                continue;
            }
            if target.is_dir() {
                if let Err(err) = crate::data::clear_dir_contents(target) {
                    tracing::warn!(path = %target.display(), error = %err, "撤销时清空目标目录失败");
                }
            }
        }
        if let Ok(mut guard) = self.pending_data_root.write() {
            *guard = None;
        }
        if let Ok(mut list) = self.pending_switches.write() {
            list.clear();
        }
        audit::event(
            &self.data_root,
            "data_root",
            "user",
            "abort",
            "success",
            serde_json::json!({ "targets": targets.iter().map(|p| p.display().to_string()).collect::<Vec<_>>() }),
        );
        tracing::info!(count = targets.len(), "data root switch aborted");
        Ok(self.data_root.clone())
    }

    /// 打开本地目录（白名单：models / data / logs）
    pub fn open_local_path(&self, kind: &str) -> Result<PathBuf> {
        let path = match kind.trim() {
            "models" => self.models_dir(),
            "data" => {
                // 已切换但未重启时，用户期望打开的是新目录
                let pending = self
                    .pending_data_root
                    .read()
                    .ok()
                    .and_then(|guard| guard.clone())
                    .filter(|path| path.is_dir());
                pending.unwrap_or_else(|| self.data_root.clone())
            }
            "logs" => self.data_root.join("logs"),
            other => {
                return Err(AppError::InvalidInput(format!(
                    "不支持的目录类型：{other}（可选 models / data / logs）"
                )))
            }
        };
        std::fs::create_dir_all(&path)?;
        // 统一成绝对路径 + 反斜杠，并去掉 `\\?\` 前缀（Explorer 不认扩展前缀）
        let absolute = path
            .canonicalize()
            .map(strip_extended_prefix)
            .unwrap_or_else(|_| path.clone());
        open_directory(&absolute)?;
        Ok(absolute)
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

    /// 保存 Provider：先验证 Key 与 Base URL，验证不通过则**不落盘**（配置与密钥都不写）
    pub async fn save_provider(&self, input: ProviderInput) -> Result<ProviderView> {
        let api_key = input.api_key.clone();
        let mut provider = providers::normalize(input)?;
        provider.name = providers::clean_name(&provider.name);

        // 本次填了新 Key 就用新的；否则用已保存的 Key 复核一次
        let effective_key = match api_key
            .as_deref()
            .map(str::trim)
            .filter(|key| !key.is_empty())
        {
            Some(key) => Some(key.to_string()),
            None => secrets::get(&self.data_root, &provider.id),
        };

        if provider.enabled {
            let Some(key) = effective_key.as_deref() else {
                return Err(AppError::InvalidInput(
                    "请先填写 API Key：保存前会验证连通性，验证失败不会写入".to_string(),
                ));
            };
            self.verify_provider_credentials(&provider, key).await?;
        }

        let mut settings = self.current_settings();
        if !settings
            .verified_providers
            .iter()
            .any(|id| id == &provider.id)
        {
            settings.verified_providers.push(provider.id.clone());
        }
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

    /// 验证 Provider：先探 `/models`（鉴权），再用默认模型发一次最小请求（模型是否存在）
    async fn verify_provider_credentials(
        &self,
        provider: &crate::settings::ProviderSettings,
        api_key: &str,
    ) -> Result<Vec<String>> {
        let provider_id = provider.id.as_str();
        let models = match self
            .providers
            .list_models(&provider.base_url, api_key)
            .await
        {
            Ok(models) => {
                tracing::info!(provider = %provider_id, models = models.len(), "provider credentials verified");
                models
            }
            Err(err) => {
                let detail = providers::classify_error(&err.to_string());
                tracing::warn!(provider = %provider_id, error = %detail, "provider verify failed (auth)");
                return Err(AppError::InvalidInput(format!(
                    "连接验证失败：{detail}。Key 未保存，请修正后重试"
                )));
            }
        };

        // 用默认模型发一次最小请求：模型名写错会在这里暴露 404，而不是等到对话时才报错
        let model = if provider.default_model.trim().is_empty() {
            provider.models.first().cloned().unwrap_or_default()
        } else {
            provider.default_model.clone()
        };
        if model.trim().is_empty() {
            return Ok(models);
        }
        if !models.is_empty() && !models.iter().any(|item| item == model.trim()) {
            tracing::warn!(provider = %provider_id, model = %model, "model not in provider catalog");
            return Err(AppError::InvalidInput(format!(
                "模型「{model}」不在该接口的模型列表里，请从列表中选择（或点「自动获取模型」）"
            )));
        }

        let probe = vec![("user".to_string(), "hi".to_string())];
        match self
            .providers
            .chat(provider, api_key, model.trim(), &probe)
            .await
        {
            Ok(_) => {
                tracing::info!(provider = %provider_id, model = %model, "model probe ok");
                Ok(models)
            }
            Err(err) => {
                let detail = providers::classify_error(&err.to_string());
                // 限流 / 欠费 / 服务端临时错误都不代表 Key 无效，放行保存（仅提示）
                if detail.contains("429") || detail.contains("402") || detail.contains("5xx") {
                    tracing::warn!(provider = %provider_id, model = %model, error = %detail, "model probe skipped (transient)");
                    return Ok(models);
                }
                tracing::warn!(provider = %provider_id, model = %model, error = %detail, "model probe failed");
                Err(AppError::InvalidInput(format!(
                    "模型「{model}」调用失败：{detail}。Key 未保存，请修正后重试"
                )))
            }
        }
    }

    /// 测试单个外部模型（1-token 最小请求），结果写回可用模型目录
    pub async fn test_external_model(&self, model_ref: &str) -> Result<String> {
        let (provider, model) = self.resolve_model_target(model_ref)?;
        let key = secrets::get(&self.data_root, &provider.id)
            .ok_or_else(|| AppError::InvalidInput("该供应商未配置 API Key".to_string()))?;
        let probe = vec![("user".to_string(), "hi".to_string())];
        let result = self.providers.chat(&provider, &key, &model, &probe).await;

        let mut settings = self.current_settings();
        let mut classified = String::new();
        if let Some(entry) = settings
            .external_models
            .iter_mut()
            .find(|item| item.id == model_ref)
        {
            match &result {
                Ok(_) => {
                    entry.verified = true;
                    entry.last_error.clear();
                }
                Err(err) => {
                    classified = providers::classify_error(&err.to_string());
                    tracing::warn!(model = %model_ref, provider = %provider.id, error = %err, "test_external_model failed");
                    entry.verified = false;
                    entry.last_error = classified.clone();
                }
            }
        }
        if let Err(err) = self.persist_settings(settings) {
            tracing::warn!(error = %err, "记录模型测试结果失败");
        }
        match result {
            Ok(_) => Ok("ok".to_string()),
            Err(_) => Err(AppError::InvalidInput(classified)),
        }
    }

    /// 把一次调用失败写回模型状态（由用户在弹窗里确认后触发）
    pub fn record_model_error(&self, model_ref: &str, message: &str) -> Result<()> {
        let mut settings = self.current_settings();
        if let Some(entry) = settings
            .external_models
            .iter_mut()
            .find(|item| item.id == model_ref)
        {
            entry.verified = false;
            entry.last_error = providers::classify_error(message);
            self.persist_settings(settings)?;
        }
        Ok(())
    }
    /// 查询 Go 套餐额度（滚动 5 小时 / 本周 / 本月）
    pub async fn go_usage(&self, provider_id: &str) -> Result<serde_json::Value> {
        let settings = self.current_settings();
        let provider = settings
            .providers
            .iter()
            .find(|item| item.id == provider_id)
            .ok_or_else(|| AppError::InvalidInput("供应商不存在".to_string()))?;
        let key = secrets::get(&self.data_root, provider_id)
            .ok_or_else(|| AppError::InvalidInput("该供应商未配置 API Key".to_string()))?;
        let url = "https://opencode.ai/zen/go/v1/usage";
        tracing::info!(provider = provider.id, key_len = key.len(), url, "go usage request");
        let value = match self.providers.get_json(url, &key).await {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!(provider = provider.id, url, error = %err, "go usage failed");
                return Err(err);
            }
        };
        tracing::info!(provider = provider.id, body = %value, "go usage ok");
        tracing::info!(provider = provider.id, "go usage fetched");
        Ok(value)
    }
    /// 用量聚合：period = today | month | total
    pub fn usage_summary(&self, period: &str, provider: Option<&str>) -> crate::usage::UsageSummary {
        let now = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_millis() as u64)
            .unwrap_or(0);
        crate::usage::summary_of(&self.data_root, period, now, provider)
    }
    /// 查询供应商余额（目前仅 DeepSeek 提供公开接口；其余返回 None）
    pub async fn provider_balance(
        &self,
        provider_id: &str,
    ) -> Result<(Option<f64>, String, String)> {
        let settings = self.current_settings();
        let provider = settings
            .providers
            .iter()
            .find(|item| item.id == provider_id)
            .ok_or_else(|| AppError::InvalidInput("供应商不存在".to_string()))?;
        let key = secrets::get(&self.data_root, provider_id)
            .ok_or_else(|| AppError::InvalidInput("该供应商未配置 API Key".to_string()))?;

        if !provider.base_url.contains("api.deepseek.com") {
            return Ok((None, String::new(), "该供应商未接入余额查询".to_string()));
        }
        let url = "https://api.deepseek.com/user/balance";
        tracing::info!(provider = provider.id, key_len = key.len(), url, "balance request");
        let value = match self.providers.get_json(url, &key).await {
            Ok(value) => value,
            Err(err) => {
                tracing::warn!(provider = provider.id, error = %err, "balance failed");
                return Err(err);
            }
        };
        let info = value
            .pointer("/balance_infos/0")
            .cloned()
            .unwrap_or(serde_json::Value::Null);
        let total = info
            .get("total_balance")
            .and_then(|item| item.as_str())
            .and_then(|text| text.parse::<f64>().ok());
        let currency = info
            .get("currency")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .to_string();
        let note = if total.is_some() {
            String::new()
        } else {
            "余额字段缺失".to_string()
        };
        Ok((total, currency, note))
    }
    /// 保存供应商显示顺序（可用模型页的卡片排序）
    pub fn set_provider_order(&self, ids: Vec<String>) -> Result<()> {
        let mut settings = self.current_settings();
        settings.provider_order = ids;
        self.persist_settings(settings)?;
        Ok(())
    }

    /// 保存某个供应商内模型的显示顺序
    pub fn set_model_order(&self, provider_id: &str, ids: Vec<String>) -> Result<()> {
        let mut settings = self.current_settings();
        settings.model_order.insert(provider_id.to_string(), ids);
        self.persist_settings(settings)?;
        Ok(())
    }
    /// 清除某个 Provider 的加密密钥（保留 Provider 配置本身）
    pub fn clear_provider_key(&self, id: &str) -> Result<ProviderView> {
        let provider = self.find_provider(id)?;
        secrets::remove(&self.data_root, id)?;
        tracing::info!(provider = %id, "provider key cleared");
        Ok(providers::view(
            &provider,
            secrets::has(&self.data_root, &provider.id),
            self.cached_health(&provider.id),
        ))
    }

    /// 解析模型引用：`providerId:modelId` 走模型目录；否则按旧的 provider id 兼容
    fn resolve_model_target(&self, model_ref: &str) -> Result<(ProviderSettings, String)> {
        let key = model_ref.trim();
        let settings = self.current_settings();
        if let Some(entry) = settings.external_models.iter().find(|item| item.id == key) {
            let provider = self.find_provider(&entry.provider_id)?;
            return Ok((provider, entry.model_id.clone()));
        }
        let provider = self.find_provider(key)?;
        let model = if provider.default_model.trim().is_empty() {
            provider.models.first().cloned().unwrap_or_default()
        } else {
            provider.default_model.clone()
        };
        Ok((provider, model))
    }

    pub fn delete_provider(&self, id: &str) -> Result<()> {
        let mut settings = self.current_settings();
        // 删除保护：默认模型若属于该供应商，先让用户改默认
        if settings.default_chat != "local" {
            let owned = settings
                .external_models
                .iter()
                .any(|item| item.provider_id == id && item.id == settings.default_chat);
            if owned {
                return Err(AppError::InvalidInput(
                    "当前默认对话模型属于该供应商，请先在「模型」里切换默认模型再删除".to_string(),
                ));
            }
        }
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
            Ok(()) => {
                let mut settings = self.current_settings();
                if !settings.verified_providers.iter().any(|item| item == id) {
                    settings.verified_providers.push(id.to_string());
                    if let Err(err) = self.persist_settings(settings) {
                        tracing::warn!(provider = %id, error = %err, "记录实测供应商失败");
                    }
                }
                (true, None)
            }
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

        // 两段式解析：优先把入参当作模型 id（providerId:modelId），
        // 回退到旧行为（入参是 provider id，用其默认模型）。
        let (provider, model) = self.resolve_model_target(provider_id)?;
        if !provider.enabled {
            return Err(AppError::InvalidInput("Provider 未启用".to_string()));
        }
        let provider_id = provider.id.clone();
        let key = secrets::get(&self.data_root, &provider_id)
            .ok_or_else(|| AppError::InvalidInput("Provider 未配置 API Key".to_string()))?;
        if model.is_empty() {
            return Err(AppError::InvalidInput("该模型未配置 model id".to_string()));
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
        if !sensitive_paths.is_empty() && !self.has_export_grant(role, &provider_id) {
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

        if self.should_write_memory(role, message) {
            if let Err(err) = self.memory.remember_from_user(role, message) {
                tracing::warn!(role, error = %err, "long-term memory update failed");
            }
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
        // 云端调用频次闸门：同一会话 1 小时内超过阈值后，未获用户放行则阻断
        if !self.cloud_allowed(&session_id) {
            let usage = self.cloud_usage_snapshot(&session_id);
            tracing::warn!(
                provider = %provider_id,
                session = %session_id,
                used = usage.used,
                limit = usage.limit,
                "cloud call blocked by window limit"
            );
            return Err(AppError::InvalidInput(format!(
                "本会话 {} 分钟内已调用云端 {} 次，需要你确认后才能继续",
                usage.window_minutes, usage.used
            )));
        }

        let mut reply = self
            .providers
            .chat(&provider, &key, &model, &request_messages)
            .await?;
        reply.role = role.to_string();
        // 记录一次云端调用（滑动窗口计数）
        self.record_cloud_call(&session_id);

        // 本地用量记账（供「费用」页聚合；离线可用）
        let now_ms = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|value| value.as_millis() as u64)
            .unwrap_or(0);
        let usage_entry = crate::usage::UsageEntry {
            ts: now_ms,
            provider_id: provider_id.to_string(),
            model_id: reply.model.clone(),
            tokens_in: reply
                .usage
                .as_ref()
                .map(|item| item.prompt_tokens)
                .filter(|value| *value > 0)
                .unwrap_or(report.estimated_tokens as u64),
            tokens_out: reply.usage.as_ref().map(|item| item.completion_tokens).unwrap_or(0),
            cache_tokens: 0,
            cost: 0.0,
        };
        if let Err(err) = crate::usage::record(&self.data_root, &usage_entry) {
            tracing::warn!(error = %err, "用量记账失败");
        }

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
        let profile = self
            .capabilities
            .read()
            .map(|guard| guard.profile.clone())
            .unwrap_or_else(|_| "small".to_string());
        roles::list(&self.app_root, &self.data_root)
            .into_iter()
            .map(|mut role| {
                role.cloud_policy_resolved = Some(role.cloud_policy.resolve(&profile));
                role
            })
            .collect()
    }

    pub fn save_role(&self, role: RoleSummary) -> Result<RoleSummary> {
        roles::save_custom(&self.data_root, role)
    }

    /// 删除自建角色：彻底删除其数据（不留归档）
    pub fn delete_role(&self, id: &str) -> Result<()> {
        roles::delete_custom(&self.data_root, id)
    }

    /// explicitOnly 角色（健康 / 心理）默认不写长期记忆，只有用户明确要求才写
    fn should_write_memory(&self, role: &str, message: &str) -> bool {
        let scope = roles::load_role(&self.app_root, &self.data_root, role)
            .map(|item| item.memory_scope)
            .unwrap_or_else(|| "full".to_string());
        if scope != "explicitOnly" {
            return true;
        }
        ["记住", "记一下", "记下", "别忘", "remember"]
            .iter()
            .any(|keyword| message.contains(keyword))
    }

    /// 按角色名生成一份专业提示词（本地 7B，使用结构化输出）
    pub async fn generate_role_prompt(
        &self,
        name: &str,
        desc: &str,
    ) -> Result<crate::roles::GeneratedRolePrompt> {
        let name = name.trim();
        if name.is_empty() {
            return Err(AppError::InvalidInput("请先填写角色名称".to_string()));
        }
        let desc = desc.trim();
        let messages = [
            ChatMessage::system(GENERATE_ROLE_PROMPT_SYSTEM),
            ChatMessage::user(format!(
                "角色名称：{name}\n一句话简介：{}\n请为该角色生成系统提示词。",
                if desc.is_empty() {
                    "（未填写）"
                } else {
                    desc
                }
            )),
        ];
        let options = ChatOptions {
            temperature: 0.3,
            num_ctx: 4096,
            num_predict: 1024,
            seed: None,
        }
        .with_determinism(self.config.testing.deterministic);

        let (content, _) = self
            .ollama
            .chat_with(
                &self.config.coordinator.model,
                &messages,
                Duration::from_secs(90),
                options,
                Some(generate_role_prompt_schema()),
            )
            .await?;

        let value = crate::render::extract_json_value(&content)
            .ok_or_else(|| AppError::InvalidInput("模型输出不是合法 JSON，请重试".to_string()))?;
        let system_prompt = value
            .get("systemPrompt")
            .and_then(|item| item.as_str())
            .unwrap_or("")
            .trim()
            .to_string();
        if system_prompt.is_empty() {
            return Err(AppError::InvalidInput(
                "模型没有生成有效提示词，请重试或手动填写".to_string(),
            ));
        }
        let guardrails = value
            .get("guardrails")
            .and_then(|item| item.as_array())
            .map(|items| {
                items
                    .iter()
                    .filter_map(|item| item.as_str())
                    .map(|item| item.trim().to_string())
                    .filter(|item| !item.is_empty())
                    .collect::<Vec<_>>()
            })
            .unwrap_or_default();
        let output_style = value
            .get("outputStyle")
            .and_then(|item| item.as_str())
            .unwrap_or("concise")
            .to_string();

        Ok(crate::roles::GeneratedRolePrompt {
            system_prompt,
            guardrails,
            output_style,
        })
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

#[cfg(test)]
mod model_option_tests {
    use super::{guess_grade, looks_like_embedding, same_model};

    #[test]
    fn detects_embedding_models_by_name() {
        assert!(looks_like_embedding("nomic-embed-text"));
        assert!(looks_like_embedding("bge-m3"));
        assert!(looks_like_embedding("snowflake-arctic-embed"));
        assert!(!looks_like_embedding("qwen25-7b"));
        assert!(!looks_like_embedding("qwen3:8b"));
    }

    #[test]
    fn guesses_grade_from_param_token() {
        assert_eq!(guess_grade("qwen25-3b"), "light");
        assert_eq!(guess_grade("qwen3:8b"), "heavy");
        assert_eq!(guess_grade("llama-0.5b"), "light");
        assert_eq!(guess_grade("some-model"), "unknown");
    }

    #[test]
    fn compares_model_names_without_latest_suffix() {
        assert!(same_model("qwen25-7b:latest", "qwen25-7b"));
        assert!(same_model("QWEN25-7B", "qwen25-7b"));
        assert!(!same_model("qwen25-3b", "qwen25-7b"));
        assert!(!same_model("qwen25-7b", ""));
    }

    #[test]
    fn resolves_models_dir_placeholders() {
        let root = std::path::Path::new(r"C:\app");
        assert_eq!(
            super::resolve_models_dir("{程序目录}/models", root),
            std::path::PathBuf::from(r"C:\app/models")
        );
        assert_eq!(
            super::resolve_models_dir("{app_root}\\models", root),
            std::path::PathBuf::from(r"C:\app\models")
        );
        assert_eq!(
            super::resolve_models_dir("   ", root),
            std::path::PathBuf::from(r"C:\app").join("models")
        );
    }

    #[test]
    fn strips_windows_extended_prefix() {
        assert_eq!(
            super::strip_extended_prefix(std::path::PathBuf::from(r"\\?\C:\data\models")),
            std::path::PathBuf::from(r"C:\data\models")
        );
        assert_eq!(
            super::strip_extended_prefix(std::path::PathBuf::from(r"C:\data")),
            std::path::PathBuf::from(r"C:\data")
        );
    }
}
