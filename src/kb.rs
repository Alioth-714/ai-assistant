//! 角色知识库：分块存储、加权搜索、规模上限。

use serde::{Deserialize, Serialize};
use std::io::Write;
use std::path::{Path, PathBuf};

/// 单库分块告警阈值
pub const KB_WARN_CHUNKS: usize = 2000;

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KbChunk {
    pub id: String,
    pub url: String,
    #[serde(default)]
    pub final_url: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub text: String,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub tags: Vec<String>,
    #[serde(default)]
    pub embedding: String,
    #[serde(default)]
    pub embedding_scale: f32,
    #[serde(default)]
    pub embedding_model: String,
    #[serde(default)]
    pub embedding_status: String,
    #[serde(default)]
    pub is_key: bool,
    #[serde(default)]
    pub trust_level: String,
    #[serde(default)]
    pub kind: String,
    #[serde(default)]
    pub extract_method: String,
    #[serde(default)]
    pub extract_quality: String,
    #[serde(default)]
    pub hash: String,
    #[serde(default)]
    pub ts: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KbHit {
    pub id: String,
    pub url: String,
    pub title: String,
    pub snippet: String,
    pub score: u32,
    pub is_key: bool,
    pub trust_level: String,
}

#[derive(Debug, Clone, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct KbStats {
    pub chunks: usize,
    pub sources: usize,
    pub warn: bool,
}

fn kb_dir(data_root: &Path, role: &str) -> PathBuf {
    data_root.join("kb").join(role)
}

fn chunks_path(data_root: &Path, role: &str) -> PathBuf {
    kb_dir(data_root, role).join("chunks.jsonl")
}

/// 读取某角色全部合法分块（忽略坏行）
pub fn load_chunks(data_root: &Path, role: &str) -> Vec<KbChunk> {
    let path = chunks_path(data_root, role);
    let Ok(text) = std::fs::read_to_string(&path) else {
        return Vec::new();
    };
    text.lines()
        .filter(|line| !line.trim().is_empty())
        .filter_map(|line| serde_json::from_str::<KbChunk>(line).ok())
        .collect()
}

/// 追加写入分块，返回写入条数；超过上限则拒绝
pub fn append_chunks(data_root: &Path, role: &str, chunks: &[KbChunk]) -> Result<usize, String> {
    if chunks.is_empty() {
        return Ok(0);
    }
    let existing = load_chunks(data_root, role).len();
    if existing >= KB_WARN_CHUNKS {
        tracing::warn!(role, existing, "知识库分块数已达告警阈值");
    }
    let dir = kb_dir(data_root, role);
    std::fs::create_dir_all(&dir).map_err(|err| err.to_string())?;
    let path = chunks_path(data_root, role);
    let mut file = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(&path)
        .map_err(|err| err.to_string())?;
    let mut written = 0;
    for chunk in chunks {
        let line = serde_json::to_string(chunk).map_err(|err| err.to_string())?;
        file.write_all(line.as_bytes()).map_err(|err| err.to_string())?;
        file.write_all(b"\n").map_err(|err| err.to_string())?;
        written += 1;
    }
    Ok(written)
}

/// 统计
pub fn stats(data_root: &Path, role: &str) -> KbStats {
    let chunks = load_chunks(data_root, role).len();
    let sources_path = kb_dir(data_root, role).join("sources.jsonl");
    let sources = std::fs::read_to_string(&sources_path)
        .map(|text| text.lines().filter(|line| !line.trim().is_empty()).count())
        .unwrap_or(0);
    KbStats {
        chunks,
        sources,
        warn: chunks >= KB_WARN_CHUNKS,
    }
}

/// 加权搜索：标题 3、摘要 2、原文 1；不走向量，零外部调用
pub fn search(data_root: &Path, role: &str, query: &str, limit: usize) -> Vec<KbHit> {
    let needle = query.trim().to_lowercase();
    if needle.is_empty() {
        return Vec::new();
    }
    let mut hits: Vec<KbHit> = Vec::new();
    for chunk in load_chunks(data_root, role) {
        let title = chunk.title.to_lowercase();
        let summary = chunk.summary.to_lowercase();
        let text = chunk.text.to_lowercase();
        let mut score = 0u32;
        if title.contains(&needle) {
            score += 3;
        }
        if summary.contains(&needle) {
            score += 2;
        }
        if text.contains(&needle) {
            score += 1;
        }
        if score == 0 {
            continue;
        }
        let snippet = snippet_of(&chunk, &needle);
        hits.push(KbHit {
            id: chunk.id.clone(),
            url: chunk.url.clone(),
            title: chunk.title.clone(),
            snippet,
            score,
            is_key: chunk.is_key,
            trust_level: chunk.trust_level,
        });
    }
    hits.sort_by(|a, b| b.score.cmp(&a.score).then(b.is_key.cmp(&a.is_key)));
    hits.truncate(limit);
    hits
}

fn snippet_of(chunk: &KbChunk, needle: &str) -> String {
    let source = if !chunk.summary.is_empty() {
        &chunk.summary
    } else {
        &chunk.text
    };
    let lower = source.to_lowercase();
    let chars: Vec<char> = source.chars().collect();
    let lower_chars: Vec<char> = lower.chars().collect();
    let mut start = 0usize;
    let needle_chars: Vec<char> = needle.chars().collect();
    if needle_chars.len() <= lower_chars.len() {
        for index in 0..=lower_chars.len() - needle_chars.len() {
            if lower_chars[index..index + needle_chars.len()] == needle_chars[..] {
                start = index.saturating_sub(40);
                break;
            }
        }
    }
    let end = (start + 160).min(chars.len());
    chars[start.min(chars.len())..end].iter().collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn chunk(id: &str, title: &str, summary: &str, text: &str) -> KbChunk {
        KbChunk {
            id: id.to_string(),
            url: "https://example.com".to_string(),
            final_url: String::new(),
            title: title.to_string(),
            text: text.to_string(),
            summary: summary.to_string(),
            tags: Vec::new(),
            embedding: String::new(),
            embedding_scale: 0.0,
            embedding_model: String::new(),
            embedding_status: "ok".to_string(),
            is_key: false,
            trust_level: "neutral".to_string(),
            kind: "web".to_string(),
            extract_method: "naive".to_string(),
            extract_quality: "ok".to_string(),
            hash: String::new(),
            ts: 0,
        }
    }

    #[test]
    fn search_weights_title_above_text() {
        let dir = std::env::temp_dir().join(format!("kb-test-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        let role = "dev";
        append_chunks(
            &dir,
            role,
            &[
                chunk("a", "RTOS 调度", "", "无关内容"),
                chunk("b", "其他", "", "RTOS 调度出现在正文"),
            ],
        )
        .unwrap();
        let hits = search(&dir, role, "rtos", 10);
        assert_eq!(hits.len(), 2);
        assert_eq!(hits[0].id, "a");
        assert_eq!(hits[0].score, 3);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn stats_counts_chunks() {
        let dir = std::env::temp_dir().join(format!("kb-stats-{}", std::process::id()));
        let _ = std::fs::remove_dir_all(&dir);
        append_chunks(&dir, "dev", &[chunk("x", "t", "", "b")]).unwrap();
        let s = stats(&dir, "dev");
        assert_eq!(s.chunks, 1);
        assert!(!s.warn);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
