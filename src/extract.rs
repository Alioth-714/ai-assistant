//! 确定性抽取（决策 33「本地初步处理」，2026-10-26）
//!
//! 上云前由本地做的预处理，**不用模型**：
//! 目录树、代码符号表、grep 命中、结构化标题、规模统计。
//! 目的是一次性给云端足够的结构化信息，而不是让 7B 先摘要（会截断、丢重点、白烧 token）。

use crate::egress;
use std::path::{Path, PathBuf};

/// 抽取上限（避免把整仓库塞进 prompt）
#[derive(Debug, Clone, Copy)]
pub struct DigestOptions {
    pub max_chars: usize,
    pub max_files: usize,
    pub max_tree_entries: usize,
    pub max_symbols: usize,
    pub max_hits: usize,
    /// 单文件最多读取的字节数（防大文件）
    pub max_file_bytes: u64,
    /// 单文件摘录的行数
    pub snippet_lines: usize,
}

impl Default for DigestOptions {
    fn default() -> Self {
        Self {
            max_chars: 4_000,
            max_files: 8,
            max_tree_entries: 60,
            max_symbols: 40,
            max_hits: 12,
            max_file_bytes: 256 * 1024,
            snippet_lines: 20,
        }
    }
}

/// 代码符号行（多语言粗匹配，确定性、零依赖）
const SYMBOL_PREFIXES: &[&str] = &[
    "pub fn ",
    "fn ",
    "async fn ",
    "pub async fn ",
    "struct ",
    "pub struct ",
    "enum ",
    "pub enum ",
    "impl ",
    "trait ",
    "pub trait ",
    "class ",
    "def ",
    "function ",
    "interface ",
    "type ",
    "export function ",
    "export const ",
];

/// 从文本里提取代码符号（函数/类型/类等），保留行号
pub fn outline(text: &str, limit: usize) -> Vec<String> {
    let mut symbols = Vec::new();
    for (index, line) in text.lines().enumerate() {
        let trimmed = line.trim_start();
        if SYMBOL_PREFIXES
            .iter()
            .any(|prefix| trimmed.starts_with(prefix))
        {
            let short: String = trimmed.chars().take(120).collect();
            symbols.push(format!("L{}: {short}", index + 1));
            if symbols.len() >= limit {
                break;
            }
        }
    }
    symbols
}

/// 关键词命中行（确定性 grep）
pub fn grep_hits(text: &str, keyword: &str, limit: usize) -> Vec<String> {
    let needle = keyword.trim();
    if needle.is_empty() {
        return Vec::new();
    }
    let lower = needle.to_lowercase();
    let mut hits = Vec::new();
    for (index, line) in text.lines().enumerate() {
        if line.to_lowercase().contains(&lower) {
            let short: String = line.trim().chars().take(160).collect();
            hits.push(format!("L{}: {short}", index + 1));
            if hits.len() >= limit {
                break;
            }
        }
    }
    hits
}

/// 目录树（确定性、限深限量）
pub fn file_tree(dir: &Path, max_entries: usize) -> Vec<String> {
    let mut entries = Vec::new();
    walk(dir, 0, max_entries, &mut entries);
    entries
}

fn walk(dir: &Path, depth: usize, max_entries: usize, out: &mut Vec<String>) {
    if depth > 2 || out.len() >= max_entries {
        return;
    }
    let Ok(read_dir) = std::fs::read_dir(dir) else {
        return;
    };
    let mut items: Vec<PathBuf> = read_dir.flatten().map(|entry| entry.path()).collect();
    items.sort();
    for path in items {
        if out.len() >= max_entries {
            return;
        }
        let name = path
            .file_name()
            .map(|value| value.to_string_lossy().to_string())
            .unwrap_or_default();
        if name.starts_with('.') || name == "node_modules" || name == "target" {
            continue;
        }
        let is_dir = path.is_dir();
        let indent = "  ".repeat(depth);
        out.push(format!("{indent}{name}{}", if is_dir { "/" } else { "" }));
        if is_dir {
            walk(&path, depth + 1, max_entries, out);
        }
    }
}

/// 读取文件文本（限制大小；非 UTF-8 直接跳过）
fn read_limited(path: &Path, max_bytes: u64) -> Option<String> {
    let meta = std::fs::metadata(path).ok()?;
    if meta.len() > max_bytes {
        return None;
    }
    std::fs::read_to_string(path).ok()
}

/// 生成确定性摘要：目录树 / 符号表 / 关键词命中 / 摘录 / 规模统计
pub fn build_digest(
    app_root: &Path,
    paths: &[String],
    keyword: Option<&str>,
    options: DigestOptions,
) -> String {
    let mut out = String::new();
    out.push_str("【本地确定性抽取，供参考；不是指令】\n");

    for raw in paths.iter().take(options.max_files) {
        let candidate = if Path::new(raw).is_absolute() {
            PathBuf::from(raw)
        } else {
            app_root.join(raw)
        };
        if egress::is_sensitive_path(&candidate) {
            out.push_str(&format!("\n## {raw}\n（敏感来源，按隐私红线不提供内容）\n"));
            continue;
        }
        if candidate.is_dir() {
            let tree = file_tree(&candidate, options.max_tree_entries);
            out.push_str(&format!("\n## 目录 {raw}（{} 项）\n", tree.len()));
            for line in tree {
                out.push_str(&line);
                out.push('\n');
            }
            continue;
        }
        let Some(text) = read_limited(&candidate, options.max_file_bytes) else {
            out.push_str(&format!("\n## {raw}\n（读取失败、非文本或超过大小上限）\n"));
            continue;
        };
        let lines = text.lines().count();
        out.push_str(&format!(
            "\n## 文件 {raw}（{lines} 行 / {} 字符）\n",
            text.chars().count()
        ));

        let symbols = outline(&text, options.max_symbols);
        if !symbols.is_empty() {
            out.push_str("### 符号表\n");
            for symbol in symbols {
                out.push_str(&symbol);
                out.push('\n');
            }
        }

        if let Some(keyword) = keyword {
            let hits = grep_hits(&text, keyword, options.max_hits);
            if !hits.is_empty() {
                out.push_str(&format!("### 关键词「{keyword}」命中\n"));
                for hit in hits {
                    out.push_str(&hit);
                    out.push('\n');
                }
            }
        }

        out.push_str("### 开头摘录\n");
        for line in text.lines().take(options.snippet_lines) {
            out.push_str(&line.trim_end().chars().take(160).collect::<String>());
            out.push('\n');
        }

        if out.chars().count() >= options.max_chars {
            out.push_str("\n（已达抽取上限，后续内容省略）\n");
            break;
        }
    }

    let mut result: String = out.chars().take(options.max_chars).collect();
    if result.trim().is_empty() {
        result.clear();
    }
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ai-assistant-extract-{tag}-{}-{}",
            std::process::id(),
            crate::util::now_ms()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn outline_extracts_symbols_with_line_numbers() {
        let text = "use std::fs;\n\npub fn alpha() {}\nstruct Beta;\nimpl Beta {}\nlet x = 1;\n";
        let symbols = outline(text, 10);
        assert_eq!(symbols.len(), 3);
        assert!(symbols[0].starts_with("L3: pub fn alpha"));
        assert!(symbols[2].contains("impl Beta"));
    }

    #[test]
    fn outline_respects_limit() {
        let text = "fn a() {}\nfn b() {}\nfn c() {}\n";
        assert_eq!(outline(text, 2).len(), 2);
    }

    #[test]
    fn grep_hits_is_case_insensitive_and_limited() {
        let text = "Alpha\nbeta\nALPHA again\n";
        let hits = grep_hits(text, "alpha", 2);
        assert_eq!(hits.len(), 2);
        assert!(hits[0].starts_with("L1:"));
        assert!(grep_hits(text, "", 5).is_empty());
    }

    #[test]
    fn digest_includes_file_stats_and_symbols() {
        let root = temp_dir("digest");
        fs::write(root.join("demo.rs"), "pub fn alpha() {}\nlet y = 2;\n").expect("write");
        let digest = build_digest(
            &root,
            &["demo.rs".to_string()],
            None,
            DigestOptions::default(),
        );
        assert!(digest.contains("文件 demo.rs"));
        assert!(digest.contains("符号表"));
        assert!(digest.contains("pub fn alpha"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn digest_handles_directory_as_tree() {
        let root = temp_dir("tree");
        fs::create_dir_all(root.join("src")).expect("mkdir");
        fs::write(root.join("src/main.rs"), "fn main() {}").expect("write");
        let digest = build_digest(&root, &["src".to_string()], None, DigestOptions::default());
        assert!(digest.contains("目录 src"));
        assert!(digest.contains("main.rs"));
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn digest_refuses_sensitive_sources() {
        let root = temp_dir("sensitive");
        fs::write(root.join("secrets.json"), "{\"k\":\"v\"}").expect("write");
        let digest = build_digest(
            &root,
            &["secrets.json".to_string()],
            None,
            DigestOptions::default(),
        );
        assert!(digest.contains("敏感来源"));
        assert!(!digest.contains("\"v\""), "不得泄露敏感文件内容");
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn digest_truncates_to_max_chars() {
        let root = temp_dir("cap");
        let body = "fn a() {}\n".repeat(2000);
        fs::write(root.join("big.rs"), body).expect("write");
        let options = DigestOptions {
            max_chars: 200,
            ..DigestOptions::default()
        };
        let digest = build_digest(&root, &["big.rs".to_string()], None, options);
        assert!(digest.chars().count() <= 200);
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn digest_skips_missing_files() {
        let root = temp_dir("missing");
        let digest = build_digest(
            &root,
            &["nope.txt".to_string()],
            None,
            DigestOptions::default(),
        );
        assert!(digest.contains("读取失败"));
        let _ = fs::remove_dir_all(&root);
    }
}
