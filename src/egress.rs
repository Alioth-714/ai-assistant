//! 出网闸门（决策 33 隐私红线，2026-10-22）
//!
//! 原则：
//! 1. **敏感内容绝不上云**——即使「仅本地」开关关闭（自动模式）也不允许，红线不随模式变化。
//! 2. **程序不得自动复制敏感内容**——只能提供查看方法，由用户自行复制；本模块只做拦截，不做搬运。
//! 3. **两层防护**：来源标记（可靠层，读取时就打标）+ 密钥形态检测（兜底层，手抄内容也能识别）。
//! 4. **会话云端预算上限**——超额自动降级为仅本地。

use serde::Serialize;
use std::path::Path;

/// 敏感文件名（精确匹配，小写比较）
const SENSITIVE_NAMES: &[&str] = &[
    ".env",
    ".netrc",
    "id_rsa",
    "id_dsa",
    "id_ecdsa",
    "id_ed25519",
    "secrets.json",
    "credentials.json",
    "credentials",
];

/// 敏感文件名前缀（小写比较）
const SENSITIVE_NAME_PREFIXES: &[&str] = &[".env.", "secrets.", "credentials.", "secrets-"];

/// 敏感扩展名
const SENSITIVE_EXTENSIONS: &[&str] = &["key", "pem", "p12", "pfx", "jks", "keystore"];

/// 敏感目录名
const SENSITIVE_DIRS: &[&str] = &[".ssh", ".aws", ".gnupg", ".config-keys"];

/// 是否为敏感来源路径（用于打 source 标记 / 出网拦截）。
pub fn is_sensitive_path(path: &Path) -> bool {
    let name = path
        .file_name()
        .map(|value| value.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let extension = path
        .extension()
        .map(|value| value.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    let name_hit = SENSITIVE_NAMES.contains(&name.as_str())
        || SENSITIVE_NAME_PREFIXES
            .iter()
            .any(|prefix| name.starts_with(prefix));
    let ext_hit = SENSITIVE_EXTENSIONS.contains(&extension.as_str());
    let dir_hit = path.components().any(|component| {
        let value = component.as_os_str().to_string_lossy().to_lowercase();
        SENSITIVE_DIRS.contains(&value.as_str())
    });

    name_hit || ext_hit || dir_hit
}

/// 密钥形态命中
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
pub struct SecretFinding {
    /// 命中类型（用于提示，不泄露内容）
    pub kind: String,
    /// 截断预览（只露头尾几位）
    pub preview: String,
}

/// 扫描文本里的密钥形态（兜底层：用户手抄内容也能识别）。
pub fn scan_secrets(text: &str) -> Vec<SecretFinding> {
    let mut findings = Vec::new();

    if let Some(token) = find_prefixed_token(text, "sk-", 16) {
        findings.push(SecretFinding {
            kind: "api-key(sk-)".to_string(),
            preview: mask(&token),
        });
    }
    if text.contains("-----BEGIN") && text.to_uppercase().contains("PRIVATE KEY") {
        findings.push(SecretFinding {
            kind: "private-key-pem".to_string(),
            preview: "-----BEGIN ... PRIVATE KEY-----".to_string(),
        });
    }
    if let Some(token) = find_prefixed_token(text, "AKIA", 16) {
        findings.push(SecretFinding {
            kind: "aws-access-key".to_string(),
            preview: mask(&token),
        });
    }
    for prefix in ["ghp_", "gho_", "github_pat_"] {
        if let Some(token) = find_prefixed_token(text, prefix, 20) {
            findings.push(SecretFinding {
                kind: "github-token".to_string(),
                preview: mask(&token),
            });
            break;
        }
    }
    for prefix in ["xoxb-", "xoxp-", "xoxa-", "xoxr-", "xoxs-"] {
        if let Some(token) = find_prefixed_token(text, prefix, 10) {
            findings.push(SecretFinding {
                kind: "slack-token".to_string(),
                preview: mask(&token),
            });
            break;
        }
    }
    if let Some(token) = find_prefixed_token(text, "AIza", 30) {
        findings.push(SecretFinding {
            kind: "google-api-key".to_string(),
            preview: mask(&token),
        });
    }

    findings
}

fn find_prefixed_token(text: &str, prefix: &str, min_len: usize) -> Option<String> {
    let mut offset = 0;
    while let Some(found) = text[offset..].find(prefix) {
        let start = offset + found;
        let rest = &text[start..];
        let token: String = rest
            .chars()
            .take_while(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
            .collect();
        if token.len() >= prefix.len() + min_len {
            return Some(token);
        }
        offset = start + prefix.len();
        if offset >= text.len() {
            break;
        }
    }
    None
}

fn mask(token: &str) -> String {
    let head: String = token.chars().take(6).collect();
    let tail: String = token
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    format!("{head}…{tail}（共 {} 字符）", token.chars().count())
}

/// 会话云端预算（token 估算，超额即降级为仅本地）
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CloudBudget {
    /// None = 不限
    pub limit_tokens: Option<u64>,
    pub used_tokens: u64,
}

impl Default for CloudBudget {
    fn default() -> Self {
        Self {
            limit_tokens: Some(50_000),
            used_tokens: 0,
        }
    }
}

impl CloudBudget {
    pub fn new(limit_tokens: Option<u64>) -> Self {
        Self {
            limit_tokens,
            used_tokens: 0,
        }
    }

    /// 粗略估算 token 数（中文约 1 字 1 token、英文约 4 字符 1 token，这里按保守的 chars/2）。
    pub fn estimate_tokens(text: &str) -> u64 {
        (text.chars().count() as u64).div_ceil(2)
    }

    pub fn remaining(&self) -> Option<u64> {
        self.limit_tokens
            .map(|limit| limit.saturating_sub(self.used_tokens))
    }

    pub fn exhausted(&self) -> bool {
        match self.limit_tokens {
            Some(limit) => self.used_tokens >= limit,
            None => false,
        }
    }

    /// 尝试消费预算；返回 false 表示超额（调用方应降级为仅本地）。
    pub fn try_consume(&mut self, tokens: u64) -> bool {
        match self.limit_tokens {
            Some(limit) if self.used_tokens.saturating_add(tokens) > limit => false,
            _ => {
                self.used_tokens = self.used_tokens.saturating_add(tokens);
                true
            }
        }
    }
}

/// 出网检查报告
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct EgressReport {
    /// 命中密钥形态
    pub findings: Vec<SecretFinding>,
    /// 内容来源是否敏感
    pub sensitive_sources: Vec<String>,
    /// 估算 token
    pub estimated_tokens: u64,
    /// 拦截原因（None = 放行）
    pub blocked: Option<String>,
}

impl EgressReport {
    pub fn allowed(&self) -> bool {
        self.blocked.is_none()
    }
}

/// 出网闸门核心：红线检查（不读模式、不读开关）。
///
/// - `text`：即将发送给云端的完整内容
/// - `sensitive_sources`：构成该内容的敏感来源（读取时打的 source 标记）
pub fn inspect(text: &str, sensitive_sources: &[String]) -> EgressReport {
    let findings = scan_secrets(text);
    let estimated_tokens = CloudBudget::estimate_tokens(text);

    let blocked = if !sensitive_sources.is_empty() {
        Some(format!(
            "内容来自敏感来源（{}），按隐私红线不得上云；如需使用，请你自己到对应位置复制，程序不代为搬运",
            sensitive_sources.join("、")
        ))
    } else {
        findings.first().map(|first| {
            format!(
                "检测到疑似密钥（{}），按隐私红线不得上云；如确需使用，请自行处理后再粘贴",
                first.kind
            )
        })
    };

    EgressReport {
        findings,
        sensitive_sources: sensitive_sources.to_vec(),
        estimated_tokens,
        blocked,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    #[test]
    fn identifies_sensitive_paths() {
        for path in [
            ".env",
            ".env.local",
            "config/secrets.json",
            "secrets.api.json",
            "keys/server.pem",
            "id_rsa",
            "certs/client.p12",
        ] {
            assert!(
                is_sensitive_path(&PathBuf::from(path)),
                "{path} 应判定为敏感"
            );
        }
        for path in ["src/main.rs", "docs/README.md", "config/app.json"] {
            assert!(
                !is_sensitive_path(&PathBuf::from(path)),
                "{path} 不应判定为敏感"
            );
        }
    }

    #[test]
    fn detects_ssh_and_aws_dirs() {
        assert!(is_sensitive_path(Path::new(r"C:\Users\x\.ssh\config")));
        assert!(is_sensitive_path(Path::new(r"C:\Users\x\.aws\credentials")));
    }

    #[test]
    fn blocks_sensitive_source_content() {
        let report = inspect("普通内容", &["config/secrets.json".to_string()]);
        assert!(!report.allowed());
        assert!(report.blocked.unwrap().contains("敏感来源"));
    }

    #[test]
    fn blocks_api_key_shape_even_without_source() {
        let text = "key=sk-abcdefghijklmnop1234567890";
        let report = inspect(text, &[]);
        assert!(!report.allowed());
        assert_eq!(report.findings[0].kind, "api-key(sk-)");
    }

    #[test]
    fn blocks_private_key_block() {
        let text = "-----BEGIN RSA PRIVATE KEY-----\nMIIE...\n-----END RSA PRIVATE KEY-----";
        let report = inspect(text, &[]);
        assert!(!report.allowed());
        assert_eq!(report.findings[0].kind, "private-key-pem");
    }

    #[test]
    fn allows_plain_content() {
        let report = inspect("帮我看下这段 Rust 代码为什么编译不过", &[]);
        assert!(report.allowed());
        assert!(report.findings.is_empty());
    }

    #[test]
    fn does_not_flag_normal_words() {
        // 普通代码里的 sk- 片段、长变量名不应误报
        let report = inspect("let task_list = build_tasks(); // sk-learn 风格", &[]);
        assert!(report.allowed(), "{:?}", report.findings);
    }

    #[test]
    fn budget_tracks_and_blocks_over_limit() {
        let mut budget = CloudBudget::new(Some(100));
        assert!(budget.try_consume(60));
        assert!(!budget.exhausted());
        assert!(!budget.try_consume(60), "超额应返回 false");
        assert_eq!(budget.used_tokens, 60, "超额不扣减");
        assert_eq!(budget.remaining(), Some(40));
    }

    #[test]
    fn budget_none_means_unlimited() {
        let mut budget = CloudBudget::new(None);
        assert!(budget.try_consume(10_000_000));
        assert!(!budget.exhausted());
        assert_eq!(budget.remaining(), None);
    }

    #[test]
    fn estimates_tokens_conservatively() {
        assert_eq!(CloudBudget::estimate_tokens(""), 0);
        assert_eq!(CloudBudget::estimate_tokens("ab"), 1);
        assert_eq!(CloudBudget::estimate_tokens("abcd"), 2);
        assert_eq!(CloudBudget::estimate_tokens("中文测试"), 2);
    }

    #[test]
    fn default_budget_has_limit() {
        assert_eq!(CloudBudget::default().limit_tokens, Some(50_000));
    }
}
