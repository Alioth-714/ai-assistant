//! 能力分层（决策 33，2026-10-22）
//!
//! 理念：**按认知复杂度分档**，而不是按"步骤多少"。
//! - 编排复杂（多步/多文件）→ 本地 Planner 用代码拼接即可，不必上云
//! - 认知复杂（长上下文理解/深度推理/高质量写作）→ 超出 7B 上限，默认云端
//!
//! 三档 grade：`local`（永不出网）/ `localFirst`（本地先试失败升级）/ `cloud`（默认云端）。
//! 判定优先级：用户「仅本地」开关 > 静态能力白名单 > 确定性信号 > 模型自评（**不参与决策**）。
//! `profile`（small/large）让"换硬件"变成配置切换，代码与角色配置不动。

use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// 能力档位
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Grade {
    Local,
    LocalFirst,
    Cloud,
}

impl Grade {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::LocalFirst => "localFirst",
            Self::Cloud => "cloud",
        }
    }

    pub fn parse(value: &str) -> Option<Self> {
        match value.trim() {
            "local" => Some(Self::Local),
            "localFirst" | "local-first" => Some(Self::LocalFirst),
            "cloud" => Some(Self::Cloud),
            _ => None,
        }
    }

    /// 升级一档（确定性信号触发），已到顶则保持。
    pub fn escalate(self) -> Self {
        match self {
            Self::Local => Self::LocalFirst,
            Self::LocalFirst | Self::Cloud => Self::Cloud,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capability {
    pub grade: Grade,
    #[serde(default)]
    pub note: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Capabilities {
    #[serde(default = "default_profile")]
    pub profile: String,
    #[serde(default)]
    pub profiles: BTreeMap<String, Profile>,
}

/// 单个能力档位（profile）：便于以后挂更多字段（如 num_ctx、模型映射）
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Profile {
    #[serde(default)]
    pub capabilities: BTreeMap<String, Capability>,
}

fn default_profile() -> String {
    "small".to_string()
}

impl Default for Capabilities {
    fn default() -> Self {
        let mut caps = Self {
            profile: default_profile(),
            profiles: BTreeMap::new(),
        };
        caps.profiles.insert(
            "small".to_string(),
            Profile {
                capabilities: small_profile(),
            },
        );
        caps.profiles.insert(
            "large".to_string(),
            Profile {
                capabilities: large_profile(),
            },
        );
        caps
    }
}

fn cap(grade: Grade, note: &str) -> Capability {
    Capability {
        grade,
        note: note.to_string(),
    }
}

fn small_profile() -> BTreeMap<String, Capability> {
    BTreeMap::from([
        ("chat_short".to_string(), cap(Grade::Local, "闲聊 / 短问答")),
        (
            "read_write_file".to_string(),
            cap(Grade::Local, "读写文件、单文件改动、模板工作流"),
        ),
        (
            "summarize_short".to_string(),
            cap(Grade::Local, "短文档 / 单段落摘要"),
        ),
        (
            "code_explain".to_string(),
            cap(Grade::LocalFirst, "函数级解释 / 注释"),
        ),
        (
            "ocr_scanned".to_string(),
            cap(Grade::LocalFirst, "扫描 PDF OCR"),
        ),
        (
            "summarize_long".to_string(),
            cap(Grade::Cloud, "长文档跨段落高质量总结"),
        ),
        (
            "code_refactor".to_string(),
            cap(Grade::Cloud, "跨文件重构 / 架构级分析"),
        ),
        (
            "workflow_open_plan".to_string(),
            cap(Grade::Cloud, "开放式多步任务拆解"),
        ),
    ])
}

fn large_profile() -> BTreeMap<String, Capability> {
    BTreeMap::from([
        ("chat_short".to_string(), cap(Grade::Local, "闲聊 / 短问答")),
        (
            "read_write_file".to_string(),
            cap(Grade::Local, "读写文件、单文件改动、模板工作流"),
        ),
        (
            "summarize_short".to_string(),
            cap(Grade::Local, "短文档 / 单段落摘要"),
        ),
        (
            "code_explain".to_string(),
            cap(Grade::Local, "函数级解释 / 注释"),
        ),
        (
            "ocr_scanned".to_string(),
            cap(Grade::LocalFirst, "扫描 PDF OCR"),
        ),
        (
            "summarize_long".to_string(),
            cap(Grade::Local, "14B+ 本地可承担"),
        ),
        (
            "code_refactor".to_string(),
            cap(Grade::LocalFirst, "14B+ 本地优先，失败再上云"),
        ),
        (
            "workflow_open_plan".to_string(),
            cap(Grade::LocalFirst, "14B+ 本地优先，失败再上云"),
        ),
    ])
}

/// 从 `{app_root}/config/capabilities.json` 加载；缺失或损坏时用内置默认（small）。
pub fn load(app_root: &Path) -> Capabilities {
    let path: PathBuf = app_root.join("config").join("capabilities.json");
    let Ok(text) = std::fs::read_to_string(&path) else {
        tracing::warn!(path = %path.display(), "capabilities.json 不可用，使用内置默认 small");
        return Capabilities::default();
    };
    match serde_json::from_str::<Capabilities>(&text) {
        Ok(caps) if !caps.profiles.is_empty() => caps,
        Ok(_) => {
            tracing::warn!("capabilities.json 缺少 profiles，使用内置默认");
            Capabilities::default()
        }
        Err(err) => {
            tracing::warn!(error = %err, "capabilities.json 解析失败，使用内置默认");
            Capabilities::default()
        }
    }
}

impl Capabilities {
    pub fn active(&self) -> &BTreeMap<String, Capability> {
        self.profiles
            .get(&self.profile)
            .or_else(|| self.profiles.get("small"))
            .or_else(|| self.profiles.values().next())
            .map(|profile| &profile.capabilities)
            .expect("至少一个 profile")
    }

    /// 查能力档位；未登记的能力按 `localFirst` 处理（保守：本地先试，不直接外发）。
    pub fn grade_for(&self, capability: &str) -> Grade {
        self.active()
            .get(capability)
            .map(|item| item.grade)
            .unwrap_or(Grade::LocalFirst)
    }

    pub fn note_for(&self, capability: &str) -> Option<&str> {
        self.active().get(capability).map(|item| item.note.as_str())
    }

    pub fn switch_profile(&mut self, name: &str) -> Result<()> {
        if !self.profiles.contains_key(name) {
            return Err(AppError::Config(format!(
                "未知能力档位 profile：{name}（可用：{}）",
                self.profiles.keys().cloned().collect::<Vec<_>>().join(", ")
            )));
        }
        self.profile = name.to_string();
        Ok(())
    }

    pub fn capability_names(&self) -> Vec<String> {
        self.active().keys().cloned().collect()
    }
}

/// 路由结果
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Route {
    Local,
    LocalFirst,
    Cloud,
    /// 仅本地模式下遇到 cloud 能力：本地跑，但提示用户"可申请云端协助"
    LocalWithCloudHint,
}

impl Route {
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Local => "local",
            Self::LocalFirst => "localFirst",
            Self::Cloud => "cloud",
            Self::LocalWithCloudHint => "localWithCloudHint",
        }
    }
}

/// 确定性信号（不依赖模型自评）
#[derive(Debug, Clone, Copy, Default)]
pub struct TaskSignals {
    /// 涉及文件数（≥2 视为编排复杂）
    pub files: usize,
    /// 输入字符数（>8000 视为长上下文）
    pub chars: usize,
}

impl TaskSignals {
    /// 是否触发升级一档
    pub fn escalated(&self) -> bool {
        self.files >= 2 || self.chars > 8_000
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RouteDecision {
    pub route: Route,
    pub capability: String,
    /// 决策依据（可审计）
    pub reason: String,
    /// 是否需要用户放权才能上云（仅本地模式下为 true）
    pub needs_cloud_consent: bool,
}

/// 能力判定：开关 > 白名单 > 确定性信号（模型自评不参与）
pub fn decide(
    caps: &Capabilities,
    local_only: bool,
    capability: &str,
    signals: &TaskSignals,
) -> RouteDecision {
    let base = caps.grade_for(capability);
    let mut grade = base;
    let mut reasons = vec![format!("白名单={}", base.as_str())];

    if signals.escalated() {
        let escalated = base.escalate();
        if escalated != base {
            grade = escalated;
            reasons.push(format!(
                "信号升级(files={}, chars={}) -> {}",
                signals.files,
                signals.chars,
                escalated.as_str()
            ));
        } else {
            reasons.push("信号已达最高档".to_string());
        }
    }

    if local_only {
        reasons.push("仅本地开关=开".to_string());
        let needs_consent = grade == Grade::Cloud;
        return RouteDecision {
            route: if needs_consent {
                Route::LocalWithCloudHint
            } else {
                Route::Local
            },
            capability: capability.to_string(),
            reason: reasons.join("；"),
            needs_cloud_consent: needs_consent,
        };
    }

    let route = match grade {
        Grade::Local => Route::Local,
        Grade::LocalFirst => Route::LocalFirst,
        Grade::Cloud => Route::Cloud,
    };
    RouteDecision {
        route,
        capability: capability.to_string(),
        reason: reasons.join("；"),
        needs_cloud_consent: false,
    }
}

/// 确定性任务分类（关键词规则，不调模型）。
/// 只用于"选能力档"，不用于判断用户意图——意图由路由 3B 负责。
pub fn classify(message: &str) -> &'static str {
    let text = message.trim();
    let has = |words: &[&str]| words.iter().any(|word| text.contains(word));

    if has(&[
        "重构",
        "架构",
        "整体设计",
        "模块划分",
        "依赖关系",
        "代码评审",
        "安全审查",
    ]) {
        return "code_refactor";
    }
    if has(&["总结", "摘要", "提炼", "归纳", "要点"]) {
        // 长文档信号交给 TaskSignals；此处先归到短摘要，由信号升级
        return "summarize_short";
    }
    if has(&["解释", "注释", "命名", "这个是做什么", "什么意思"]) {
        return "code_explain";
    }
    if has(&[
        "读取",
        "读一下",
        "看看",
        "打开",
        "修改",
        "改一下",
        "写入",
        "替换",
    ]) {
        return "read_write_file";
    }
    if has(&["规划", "计划", "步骤", "先做", "然后", "再帮"]) {
        return "workflow_open_plan";
    }
    "chat_short"
}

#[cfg(test)]
mod tests {
    use super::*;

    fn caps_small() -> Capabilities {
        Capabilities::default()
    }

    #[test]
    fn parses_grades_and_aliases() {
        assert_eq!(Grade::parse("local"), Some(Grade::Local));
        assert_eq!(Grade::parse("localFirst"), Some(Grade::LocalFirst));
        assert_eq!(Grade::parse("local-first"), Some(Grade::LocalFirst));
        assert_eq!(Grade::parse("cloud"), Some(Grade::Cloud));
        assert_eq!(Grade::parse("weird"), None);
    }

    #[test]
    fn escalate_caps_at_cloud() {
        assert_eq!(Grade::Local.escalate(), Grade::LocalFirst);
        assert_eq!(Grade::LocalFirst.escalate(), Grade::Cloud);
        assert_eq!(Grade::Cloud.escalate(), Grade::Cloud);
    }

    #[test]
    fn default_small_profile_grades() {
        let caps = caps_small();
        assert_eq!(caps.profile, "small");
        assert_eq!(caps.grade_for("read_write_file"), Grade::Local);
        assert_eq!(caps.grade_for("code_refactor"), Grade::Cloud);
        assert_eq!(caps.grade_for("code_explain"), Grade::LocalFirst);
        // 未登记能力保守处理
        assert_eq!(caps.grade_for("unknown_cap"), Grade::LocalFirst);
    }

    #[test]
    fn large_profile_shifts_workload_local() {
        let mut caps = caps_small();
        caps.switch_profile("large").expect("switch profile");
        assert_eq!(caps.grade_for("summarize_long"), Grade::Local);
        assert_eq!(caps.grade_for("code_refactor"), Grade::LocalFirst);
        assert_eq!(caps.profile, "large");
    }

    #[test]
    fn switch_to_unknown_profile_fails() {
        let mut caps = caps_small();
        assert!(caps.switch_profile("huge").is_err());
    }

    #[test]
    fn local_only_never_routes_to_cloud() {
        let caps = caps_small();
        let decision = decide(&caps, true, "code_refactor", &TaskSignals::default());
        assert_eq!(decision.route, Route::LocalWithCloudHint);
        assert!(decision.needs_cloud_consent, "复杂任务需用户放权");
    }

    #[test]
    fn auto_mode_routes_complex_task_to_cloud() {
        let caps = caps_small();
        let decision = decide(&caps, false, "code_refactor", &TaskSignals::default());
        assert_eq!(decision.route, Route::Cloud);
        assert!(!decision.needs_cloud_consent);
    }

    #[test]
    fn simple_task_stays_local_even_in_auto_mode() {
        let caps = caps_small();
        let decision = decide(&caps, false, "read_write_file", &TaskSignals::default());
        assert_eq!(decision.route, Route::Local);
    }

    #[test]
    fn deterministic_signals_escalate_one_notch() {
        let caps = caps_small();
        let signals = TaskSignals {
            files: 3,
            chars: 100,
        };
        // code_explain 是 localFirst，信号升级到 cloud
        let decision = decide(&caps, false, "code_explain", &signals);
        assert_eq!(decision.route, Route::Cloud);
        assert!(decision.reason.contains("信号升级"));
    }

    #[test]
    fn signals_do_not_escalate_cloud_beyond_cloud() {
        let caps = caps_small();
        let signals = TaskSignals {
            files: 9,
            chars: 90_000,
        };
        let decision = decide(&caps, false, "code_refactor", &signals);
        assert_eq!(decision.route, Route::Cloud);
    }

    #[test]
    fn local_only_keeps_simple_tasks_plain_local() {
        let caps = caps_small();
        let decision = decide(&caps, true, "read_write_file", &TaskSignals::default());
        assert_eq!(decision.route, Route::Local);
        assert!(!decision.needs_cloud_consent);
    }

    #[test]
    fn classify_picks_sensible_capability() {
        assert_eq!(classify("帮我重构一下这个模块"), "code_refactor");
        assert_eq!(classify("解释一下这个函数"), "code_explain");
        assert_eq!(classify("读一下 a.txt 然后改第 3 行"), "read_write_file");
        assert_eq!(classify("你好"), "chat_short");
    }

    #[test]
    fn loads_embedded_default_when_file_missing() {
        let caps = load(Path::new("definitely-not-a-real-root"));
        assert_eq!(caps.profile, "small");
        assert_eq!(caps.grade_for("code_refactor"), Grade::Cloud);
    }

    #[test]
    fn repo_config_parses_strictly_without_fallback() {
        // 回归防护：配置文件写错时 load() 会静默回退内置默认，
        // 因此这里直接严格反序列化，确保真实文件本身是合法的。
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("config")
            .join("capabilities.json");
        let text = std::fs::read_to_string(&path).expect("读取仓库内 capabilities.json");
        let caps: Capabilities = serde_json::from_str(&text)
            .expect("capabilities.json 必须能被严格解析，否则会静默回退默认值");
        assert_eq!(caps.profile, "small");
        assert!(caps.profiles.contains_key("large"));
        assert_eq!(caps.grade_for("code_refactor"), Grade::Cloud);
    }

    #[test]
    fn loads_real_config_file_from_repo() {
        // 防止 config/capabilities.json 写错时静默回退到内置默认
        let caps = load(Path::new(env!("CARGO_MANIFEST_DIR")));
        assert_eq!(caps.profile, "small", "仓库内 profile 应为 small");
        assert!(
            caps.profiles.contains_key("large"),
            "必须同时提供 large profile 供升级硬件后切换"
        );
        assert_eq!(caps.grade_for("read_write_file"), Grade::Local);
        assert_eq!(caps.grade_for("code_refactor"), Grade::Cloud);
        assert!(caps.capability_names().len() >= 8);
    }
}
