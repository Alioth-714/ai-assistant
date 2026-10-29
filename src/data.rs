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

/// 解析顺序：指针文件（用户在界面里选的）→ `AI_ASSISTANT_DATA` → 调试默认 → `%LOCALAPPDATA%` → `$HOME` → 兜底
///
/// 注意：指针优先于环境变量，否则界面里切换过的目录会被一个陈旧的环境变量永久覆盖。
pub fn root(app_root: &Path) -> PathBuf {
    if let Some(pointer) = read_pointer() {
        if pointer.is_dir() {
            return pointer;
        }
        // 指针指向的目录被删掉时不要卡死，继续往下回退
        tracing::warn!(
            path = %pointer.display(),
            "数据目录指针指向的目录不存在，回退到默认数据目录"
        );
    }
    if let Ok(value) = std::env::var("AI_ASSISTANT_DATA") {
        if !value.trim().is_empty() {
            return PathBuf::from(value);
        }
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

/// 当前数据目录来源（写进启动日志，便于排查"切换不生效"）
pub fn root_source() -> &'static str {
    if read_pointer()
        .map(|pointer| pointer.is_dir())
        .unwrap_or(false)
    {
        return "pointer";
    }
    if std::env::var("AI_ASSISTANT_DATA")
        .map(|value| !value.trim().is_empty())
        .unwrap_or(false)
    {
        return "env";
    }
    if cfg!(debug_assertions) {
        return "debug-default";
    }
    "default"
}

/// 待清理标记文件名（切换数据目录成功后写入新目录，重启时清理旧目录）
const PENDING_CLEANUP: &str = "pending_cleanup.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingCleanup {
    path: String,
}

/// 记录"重启后删除旧数据目录"。运行期不删，避免正在使用旧目录的组件写入失败。
pub fn mark_pending_cleanup(new_root: &Path, old_root: &Path) -> Result<PathBuf> {
    fs::create_dir_all(new_root)?;
    let file = new_root.join(PENDING_CLEANUP);
    let marker = PendingCleanup {
        path: old_root.display().to_string(),
    };
    fs::write(&file, serde_json::to_string_pretty(&marker)?)?;
    tracing::info!(
        old = %old_root.display(),
        marker = %file.display(),
        "pending cleanup marked"
    );
    Ok(file)
}

/// 新数据目录看起来是否已经有内容（有设置文件，或 memory/roles 下有东西）
fn root_looks_populated(root: &Path) -> bool {
    if root.join("settings.json").is_file() {
        return true;
    }
    for child in ["memory", "roles", "knowledge"] {
        if fs::read_dir(root.join(child))
            .map(|mut entries| entries.next().is_some())
            .unwrap_or(false)
        {
            return true;
        }
    }
    false
}

/// 待迁移标记文件名：切换只登记意图，下次启动时执行搬运
const PENDING_MOVE: &str = "pending_move.json";

#[derive(Debug, Clone, Serialize, Deserialize)]
struct PendingMove {
    from: String,
}

/// 在新目录里记录"启动时把 from 的内容搬过来"
pub fn mark_pending_move(target: &Path, from: &Path) -> Result<PathBuf> {
    fs::create_dir_all(target)?;
    let file = target.join(PENDING_MOVE);
    fs::write(
        &file,
        serde_json::to_string_pretty(&PendingMove {
            from: from.display().to_string(),
        })?,
    )?;
    tracing::info!(
        from = %from.display(),
        to = %target.display(),
        "pending move marked"
    );
    Ok(file)
}

/// 撤销切换或清理残留时，删除这些目录里的迁移/清理标记
pub fn clear_pending_markers(dirs: &[PathBuf]) {
    for dir in dirs {
        let _ = fs::remove_file(dir.join(PENDING_MOVE));
        let _ = fs::remove_file(dir.join(PENDING_CLEANUP));
    }
}

/// 启动时消费"待迁移"标记，返回旧数据目录
pub fn take_pending_move(root: &Path) -> Option<PathBuf> {
    let file = root.join(PENDING_MOVE);
    let text = fs::read_to_string(&file).ok()?;
    let marker: PendingMove = serde_json::from_str(&text).ok()?;
    let _ = fs::remove_file(&file);
    let from_text = marker.from.trim();
    if from_text.is_empty() {
        return None;
    }
    let from = PathBuf::from(from_text);
    if !from.is_dir() {
        tracing::warn!(from = %from.display(), "待迁移的旧目录不存在，跳过");
        return None;
    }
    Some(from)
}

/// 把旧目录内容搬到新目录：同盘 move（瞬时），跨盘或占用则复制，搬完清空旧目录（保留文件夹）
///
/// 返回搬运的文件数量与字节数，便于界面提示。
pub fn migrate_entries(from: &Path, to: &Path) -> Result<(u64, u64)> {
    if !from.is_dir() {
        return Ok((0, 0));
    }
    fs::create_dir_all(to)?;
    let (mut files, mut bytes) = (0u64, 0u64);
    let mut failures = 0usize;

    for entry in fs::read_dir(from)? {
        let Ok(entry) = entry else {
            failures += 1;
            continue;
        };
        let src = entry.path();
        let dst = to.join(entry.file_name());
        if dst.exists() {
            if src.is_dir() && dst.is_dir() {
                // 目标已有同名目录（如 ensure_layout 建的空目录）：递归合并
                let (nested_files, nested_bytes) = migrate_entries(&src, &dst)?;
                files += nested_files;
                bytes += nested_bytes;
                let _ = fs::remove_dir(&src);
                continue;
            }
            tracing::warn!(path = %dst.display(), "目标已存在同名文件，保留新目录版本");
            failures += 1;
            continue;
        }
        if fs::rename(&src, &dst).is_ok() {
            // 同盘移动到瞬时完成；统计按真实文件数
            let (moved_files, moved_bytes) = if dst.is_dir() {
                estimate_tree(&dst).unwrap_or((1, 0))
            } else {
                (1, file_len(&dst))
            };
            files += moved_files;
            bytes += moved_bytes;
            continue;
        }
        // 跨盘或文件被占用：退回复制
        match copy_recursive(&src, &dst) {
            Ok((copied_files, copied_bytes)) => {
                files += copied_files;
                bytes += copied_bytes;
                let removed = if src.is_dir() {
                    fs::remove_dir_all(&src)
                } else {
                    fs::remove_file(&src)
                };
                let _ = removed;
            }
            Err(err) => {
                tracing::warn!(path = %src.display(), error = %err, "迁移条目失败，保留在旧目录");
                failures += 1;
            }
        }
    }

    if failures == 0 {
        // 全部搬完：清空旧目录内容，但保留文件夹本身
        if let Err(err) = clear_dir_contents(from) {
            tracing::warn!(path = %from.display(), error = %err, "清空旧目录失败");
        }
    } else {
        tracing::warn!(
            path = %from.display(),
            failures,
            "部分内容未能迁移，旧目录保留"
        );
    }
    Ok((files, bytes))
}

/// 估算目录里的文件数量与总字节（切换前给用户看个大概）
pub fn estimate_tree(root: &Path) -> Result<(u64, u64)> {
    if !root.is_dir() {
        return Ok((0, 0));
    }
    let (mut files, mut bytes) = (0u64, 0u64);
    for entry in fs::read_dir(root)? {
        let Ok(entry) = entry else { continue };
        let path = entry.path();
        if path.is_dir() {
            let (nested_files, nested_bytes) = estimate_tree(&path)?;
            files += nested_files;
            bytes += nested_bytes;
        } else {
            files += 1;
            bytes += file_len(&path);
        }
    }
    Ok((files, bytes))
}

fn file_len(path: &Path) -> u64 {
    if path.is_dir() {
        0
    } else {
        fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
    }
}

/// 递归复制（跨盘迁移用）
fn copy_recursive(src: &Path, dst: &Path) -> Result<(u64, u64)> {
    let meta = fs::metadata(src)?;
    if meta.is_dir() {
        fs::create_dir_all(dst)?;
        let (mut files, mut bytes) = (0u64, 0u64);
        for entry in fs::read_dir(src)? {
            let entry = entry?;
            let (f, b) = copy_recursive(&entry.path(), &dst.join(entry.file_name()))?;
            files += f;
            bytes += b;
        }
        Ok((files, bytes))
    } else {
        fs::copy(src, dst)?;
        Ok((1, meta.len()))
    }
}

/// 清空目录内容但保留目录本身（切换数据目录后清理旧目录用）
///
/// 单个文件删不掉不阻塞其余清理，只要有失败就返回错误，让标记保留、下次启动重试。
pub fn clear_dir_contents(dir: &Path) -> Result<()> {
    let mut failures = 0usize;
    for entry in fs::read_dir(dir)? {
        let Ok(entry) = entry else {
            failures += 1;
            continue;
        };
        let path = entry.path();
        let is_dir = entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false);
        let removed = if is_dir {
            fs::remove_dir_all(&path)
        } else {
            fs::remove_file(&path)
        };
        if let Err(err) = removed {
            tracing::warn!(path = %path.display(), error = %err, "clear entry failed");
            failures += 1;
        }
    }
    if failures > 0 {
        return Err(AppError::Io(std::io::Error::other(format!(
            "{failures} 个条目未能清理，将在下次启动重试"
        ))));
    }
    Ok(())
}

/// 启动时执行待清理：清空切换前的旧数据目录内容，但保留目录本身
///
/// 安全约束：只处理"标记里记录的、且与当前数据目录互不嵌套"的目录；
/// 清理失败保留标记，下次启动再试，绝不阻塞启动。
pub fn run_pending_cleanup(root: &Path) -> Option<PathBuf> {
    let file = root.join(PENDING_CLEANUP);
    let text = fs::read_to_string(&file).ok()?;
    let marker: PendingCleanup = serde_json::from_str(&text).ok()?;
    let target = PathBuf::from(marker.path.trim());
    if marker.path.trim().is_empty() {
        let _ = fs::remove_file(&file);
        return None;
    }

    let root_canonical = root.canonicalize().unwrap_or_else(|_| root.to_path_buf());
    let target_canonical = target
        .canonicalize()
        .unwrap_or_else(|_| target.to_path_buf());

    // 防御：不删当前目录、不删上级目录、不删盘符根
    if target_canonical == root_canonical
        || root_canonical.starts_with(&target_canonical)
        || target_canonical.parent().is_none()
    {
        tracing::warn!(
            target = %target.display(),
            "refuse to delete unsafe pending cleanup target"
        );
        let _ = fs::remove_file(&file);
        return None;
    }
    if !target_canonical.exists() {
        let _ = fs::remove_file(&file);
        return None;
    }

    // 保险：新目录看起来是空的就不清旧目录，避免"迁移没成功却把原数据清了"
    if !root_looks_populated(root) {
        tracing::warn!(
            root = %root.display(),
            old = %target_canonical.display(),
            "新数据目录看起来是空的，跳过旧目录清理"
        );
        let _ = fs::remove_file(&file);
        return None;
    }

    match clear_dir_contents(&target_canonical) {
        Ok(()) => {
            tracing::info!(
                target = %target_canonical.display(),
                "old data directory cleared after switch (folder kept)"
            );
            let _ = fs::remove_file(&file);
            Some(target_canonical)
        }
        Err(err) => {
            tracing::warn!(
                target = %target_canonical.display(),
                error = %err,
                "clean up old data directory failed, will retry next start"
            );
            None
        }
    }
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

    #[test]
    fn pending_cleanup_clears_children_but_keeps_folder() {
        let dir = temp_dir("cleanup");
        let old_root = dir.join("old-data");
        let new_root = dir.join("new-data");
        fs::create_dir_all(old_root.join("memory")).expect("mkdir old");
        fs::write(old_root.join("settings.json"), b"{}").expect("write");
        fs::create_dir_all(&new_root).expect("mkdir new");
        // 新目录必须"看起来有内容"，否则会被保险逻辑跳过
        fs::write(new_root.join("settings.json"), b"{}").expect("write new settings");

        mark_pending_cleanup(&new_root, &old_root).expect("mark");
        assert!(new_root.join(PENDING_CLEANUP).exists());
        let expected = old_root.canonicalize().unwrap_or_else(|_| old_root.clone());

        let removed = run_pending_cleanup(&new_root).expect("should clean");
        assert_eq!(removed, expected);
        assert!(old_root.exists(), "旧目录本身必须保留");
        assert_eq!(
            fs::read_dir(&old_root).expect("read").count(),
            0,
            "旧目录内容应被清空"
        );
        assert!(
            !new_root.join(PENDING_CLEANUP).exists(),
            "标记应在成功后删除"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pending_move_round_trip() {
        let dir = temp_dir("pending-move");
        let from = dir.join("old");
        let to = dir.join("new");
        fs::create_dir_all(&from).expect("mkdir old");

        mark_pending_move(&to, &from).expect("mark");
        assert!(to.join(PENDING_MOVE).exists());

        let taken = take_pending_move(&to).expect("take");
        assert_eq!(taken, from);
        assert!(!to.join(PENDING_MOVE).exists(), "消费后应删除标记");
        assert!(take_pending_move(&to).is_none(), "重复消费应返回 None");
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn migrate_entries_moves_data_and_clears_old_folder() {
        let dir = temp_dir("move-entries");
        let from = dir.join("old");
        let to = dir.join("new");
        fs::create_dir_all(from.join("memory")).expect("mkdir");
        fs::write(from.join("memory/sess.json"), b"abc").expect("write");
        fs::write(from.join("settings.json"), b"{}").expect("write");
        fs::create_dir_all(&to).expect("mkdir new");

        let (files, bytes) = migrate_entries(&from, &to).expect("migrate");
        assert_eq!(files, 2, "应搬走两个文件");
        assert!(bytes >= 5, "字节数应大于 0");
        assert!(to.join("settings.json").exists());
        assert!(to.join("memory/sess.json").exists());
        assert!(from.exists(), "旧目录本身必须保留");
        assert_eq!(
            fs::read_dir(&from).expect("read").count(),
            0,
            "旧目录内容应被清空"
        );
        let _ = fs::remove_dir_all(&dir);
    }

    #[test]
    fn pending_cleanup_refuses_ancestor_of_current_root() {
        let dir = temp_dir("cleanup-unsafe");
        let new_root = dir.join("data");
        fs::create_dir_all(&new_root).expect("mkdir");
        // 恶意/错误标记：指向当前目录的上级
        let marker = PendingCleanup {
            path: dir.display().to_string(),
        };
        fs::write(
            new_root.join(PENDING_CLEANUP),
            serde_json::to_string(&marker).expect("json"),
        )
        .expect("write marker");

        assert!(run_pending_cleanup(&new_root).is_none(), "不得删除上级目录");
        assert!(dir.exists(), "上级目录必须还在");
        let _ = fs::remove_dir_all(&dir);
    }
}
