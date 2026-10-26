use crate::error::{AppError, Result};
use serde::{Deserialize, Serialize};
use std::fs;
use std::path::{Path, PathBuf};

pub const LAYOUT: &[&str] = &[
    "config",
    "knowledge",
    "knowledge/shared",
    "knowledge/general",
    "memory",
    "memory/shared",
    "memory/dev",
    "memory/general",
    "workspace",
    "workspace/.locks",
    "backups",
    "cache",
    "cache/external",
    "cache/role_contexts",
    "cache/model_responses",
    "logs",
    "roles",
];

/// 逐字节校验的大小上限：超过这个大小只比较长度（避免把大文件读两遍）
const BYTE_VERIFY_LIMIT: u64 = 4 * 1024 * 1024;

/// 指针文件内容
#[derive(Debug, Clone, Serialize, Deserialize)]
struct DataPointer {
    path: String,
}

/// 指针文件位置：`%APPDATA%\ai-assistant\data-location.json`
pub fn pointer_path() -> Option<PathBuf> {
    let base = std::env::var("APPDATA")
        .ok()
        .filter(|value| !value.trim().is_empty())
        .map(PathBuf::from)
        .or_else(|| {
            std::env::var("LOCALAPPDATA")
                .ok()
                .filter(|value| !value.trim().is_empty())
                .map(PathBuf::from)
        })?;
    Some(base.join("ai-assistant").join("data-location.json"))
}

/// 读取指针文件指向的数据目录
pub fn read_pointer() -> Option<PathBuf> {
    let file = pointer_path()?;
    let text = fs::read_to_string(file).ok()?;
    let pointer: DataPointer = serde_json::from_str(&text).ok()?;
    let path = pointer.path.trim();
    if path.is_empty() {
        return None;
    }
    Some(PathBuf::from(path))
}

/// 写入指针文件（迁移成功后调用）
pub fn write_pointer(target: &Path) -> Result<PathBuf> {
    let file = pointer_path().ok_or_else(|| {
        AppError::Config("无法确定指针文件位置（缺少 APPDATA / LOCALAPPDATA）".to_string())
    })?;
    if let Some(parent) = file.parent() {
        fs::create_dir_all(parent)?;
    }
    let body = serde_json::to_string_pretty(&DataPointer {
        path: target.display().to_string(),
    })?;
    fs::write(&file, body)?;
    Ok(file)
}

/// 解析顺序：`AI_ASSISTANT_DATA` → 指针文件 → 调试默认 `app_root/data` → `%LOCALAPPDATA%` → `$HOME` → 兜底
pub fn root(app_root: &Path) -> PathBuf {
    if let Ok(value) = std::env::var("AI_ASSISTANT_DATA") {
        if !value.trim().is_empty() {
            return PathBuf::from(value);
        }
    }
    if let Some(pointer) = read_pointer() {
        return pointer;
    }
    if cfg!(debug_assertions) {
        return app_root.join("data");
    }

    if let Ok(local) = std::env::var("LOCALAPPDATA") {
        if !local.trim().is_empty() {
            return PathBuf::from(local).join("ai-assistant-data");
        }
    }
    if let Ok(home) = std::env::var("HOME") {
        if !home.trim().is_empty() {
            return PathBuf::from(home)
                .join(".local")
                .join("share")
                .join("ai-assistant")
                .join("data");
        }
    }
    app_root.join("data")
}

pub fn ensure_layout(root: &Path) -> Result<()> {
    std::fs::create_dir_all(root)?;
    for dir in LAYOUT {
        std::fs::create_dir_all(root.join(dir))?;
    }
    Ok(())
}

/// 迁移报告
#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct MigrationReport {
    pub source: String,
    pub target: String,
    pub files: usize,
    pub bytes: u64,
    /// 是否全部通过校验
    pub verified: bool,
    /// 无法复制而跳过的文件数（例如被占用的日志）
    pub skipped: usize,
}

/// 把 `source` 的全部内容复制到 `target` 并校验。
/// 校验策略：小文件逐字节比较，大文件比较长度。
pub fn migrate(source: &Path, target: &Path) -> Result<MigrationReport> {
    let source = source
        .canonicalize()
        .unwrap_or_else(|_| source.to_path_buf());
    if !source.exists() {
        return Ok(MigrationReport {
            source: source.display().to_string(),
            target: target.display().to_string(),
            files: 0,
            bytes: 0,
            verified: true,
            skipped: 0,
        });
    }
    fs::create_dir_all(target)?;
    let target_canonical = target
        .canonicalize()
        .unwrap_or_else(|_| target.to_path_buf());
    if target_canonical.starts_with(&source) || source.starts_with(&target_canonical) {
        return Err(AppError::InvalidInput(
            "目标目录不能与当前数据目录互相嵌套".to_string(),
        ));
    }

    let mut report = MigrationReport {
        source: source.display().to_string(),
        target: target.display().to_string(),
        files: 0,
        bytes: 0,
        verified: true,
        skipped: 0,
    };
    copy_tree(&source, &source, target, &mut report)?;
    Ok(report)
}

fn copy_tree(
    root: &Path,
    current: &Path,
    target_root: &Path,
    report: &mut MigrationReport,
) -> Result<()> {
    let entries = match fs::read_dir(current) {
        Ok(entries) => entries,
        Err(err) => {
            tracing::warn!(path = %current.display(), error = %err, "读取源目录失败，跳过");
            report.skipped += 1;
            return Ok(());
        }
    };
    for entry in entries.flatten() {
        let path = entry.path();
        let Ok(relative) = path.strip_prefix(root) else {
            continue;
        };
        let destination = target_root.join(relative);
        if path.is_dir() {
            fs::create_dir_all(&destination)?;
            copy_tree(root, &path, target_root, report)?;
            continue;
        }
        let Ok(meta) = fs::metadata(&path) else {
            report.skipped += 1;
            continue;
        };
        if let Some(parent) = destination.parent() {
            fs::create_dir_all(parent)?;
        }
        if let Err(err) = fs::copy(&path, &destination) {
            tracing::warn!(path = %path.display(), error = %err, "复制失败，跳过该文件");
            report.skipped += 1;
            continue;
        }
        // 校验
        let verified = verify_file(&path, &destination, meta.len());
        if !verified {
            report.verified = false;
            tracing::warn!(path = %path.display(), "迁移后校验不一致");
        }
        report.files += 1;
        report.bytes += meta.len();
    }
    Ok(())
}

fn verify_file(source: &Path, target: &Path, size: u64) -> bool {
    let Ok(target_meta) = fs::metadata(target) else {
        return false;
    };
    if target_meta.len() != size {
        return false;
    }
    if size > BYTE_VERIFY_LIMIT {
        return true; // 大文件只比长度
    }
    match (fs::read(source), fs::read(target)) {
        (Ok(left), Ok(right)) => left == right,
        _ => false,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_dir(tag: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ai-assistant-data-{tag}-{}-{}",
            std::process::id(),
            crate::util::now_ms()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[test]
    fn ensure_layout_creates_expected_dirs() {
        let root = temp_dir("layout");
        ensure_layout(&root).expect("ensure layout");
        for dir in LAYOUT {
            assert!(root.join(dir).is_dir(), "{dir} 应存在");
        }
        let _ = fs::remove_dir_all(&root);
    }

    #[test]
    fn migrate_copies_tree_and_verifies() {
        let source = temp_dir("src");
        let target = temp_dir("dst");
        fs::create_dir_all(source.join("workspace")).expect("mkdir");
        fs::write(source.join("workspace/a.txt"), "hello").expect("write");
        fs::write(source.join("top.txt"), "top level").expect("write");

        let report = migrate(&source, &target).expect("migrate");
        assert_eq!(report.files, 2);
        assert!(report.verified);
        assert_eq!(report.skipped, 0);
        assert_eq!(
            fs::read_to_string(target.join("workspace/a.txt")).unwrap(),
            "hello"
        );
        assert_eq!(
            fs::read_to_string(target.join("top.txt")).unwrap(),
            "top level"
        );

        let _ = fs::remove_dir_all(&source);
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn migrate_rejects_nested_target() {
        let source = temp_dir("nested");
        let nested = source.join("inner");
        assert!(migrate(&source, &nested).is_err());
        let _ = fs::remove_dir_all(&source);
    }

    #[test]
    fn migrate_missing_source_is_noop() {
        let root = temp_dir("missing");
        let target = temp_dir("missing-dst");
        let report = migrate(&root.join("absent"), &target).expect("migrate");
        assert_eq!(report.files, 0);
        assert!(report.verified);
        let _ = fs::remove_dir_all(&root);
        let _ = fs::remove_dir_all(&target);
    }

    #[test]
    fn verify_file_rejects_size_mismatch() {
        let dir = temp_dir("verify");
        let a = dir.join("a.txt");
        let b = dir.join("b.txt");
        fs::write(&a, "12345").expect("write a");
        fs::write(&b, "1234").expect("write b");
        assert!(!verify_file(&a, &b, 5));
        fs::write(&b, "12345").expect("write b again");
        assert!(verify_file(&a, &b, 5));
        let _ = fs::remove_dir_all(&dir);
    }
}
