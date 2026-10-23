use crate::error::{AppError, Result};
use crate::util::now_ms;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::time::Duration;

const LOCK_TTL_MS: u64 = 5 * 60 * 1000;
const ACQUIRE_ATTEMPTS: usize = 5;
const RETRY_DELAY: Duration = Duration::from_millis(500);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct LockInfo {
    pub file_path: String,
    pub holder: String,
    pub task_id: String,
    pub acquired_at: u64,
    pub expires_at: u64,
}

pub struct FileLock {
    path: PathBuf,
}

impl FileLock {
    pub async fn acquire(
        lock_dir: &Path,
        data_root: &Path,
        target: &Path,
        holder: &str,
        task_id: &str,
    ) -> Result<Self> {
        Self::acquire_with(
            lock_dir,
            data_root,
            target,
            holder,
            task_id,
            ACQUIRE_ATTEMPTS,
            RETRY_DELAY,
        )
        .await
    }

    async fn acquire_with(
        lock_dir: &Path,
        data_root: &Path,
        target: &Path,
        holder: &str,
        task_id: &str,
        attempts: usize,
        retry_delay: Duration,
    ) -> Result<Self> {
        fs::create_dir_all(lock_dir)?;
        let mut last_holder = "unknown".to_string();
        let attempts = attempts.max(1);

        for attempt in 0..attempts {
            match try_acquire_once(lock_dir, target, holder, task_id)? {
                TryOutcome::Acquired(lock) => {
                    tracing::debug!(
                        target = %target.display(),
                        holder,
                        task_id,
                        "file lock acquired"
                    );
                    return Ok(lock);
                }
                TryOutcome::StaleRemoved => continue,
                TryOutcome::Busy(busy_holder) => {
                    last_holder = busy_holder;
                    if attempt + 1 < attempts && !retry_delay.is_zero() {
                        tokio::time::sleep(retry_delay).await;
                    }
                }
            }
        }

        crate::audit::event(
            data_root,
            "file_lock_conflict",
            holder,
            "write_file",
            "blocked",
            json!({
                "target": target.display().to_string(),
                "holder": last_holder,
                "task_id": task_id
            }),
        );
        Err(AppError::InvalidInput(format!(
            "FILE_LOCKED：{} 正被 {last_holder} 写入，请稍后重试",
            target.display()
        )))
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for FileLock {
    fn drop(&mut self) {
        let _ = fs::remove_file(&self.path);
        tracing::debug!(path = %self.path.display(), "file lock released");
    }
}

enum TryOutcome {
    Acquired(FileLock),
    StaleRemoved,
    Busy(String),
}

fn try_acquire_once(
    lock_dir: &Path,
    target: &Path,
    holder: &str,
    task_id: &str,
) -> Result<TryOutcome> {
    let lock_path = lock_dir.join(format!("{}.lock.json", lock_name(target)));
    let now = now_ms();

    match OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&lock_path)
    {
        Ok(mut file) => {
            let info = LockInfo {
                file_path: target.display().to_string(),
                holder: holder.to_string(),
                task_id: task_id.to_string(),
                acquired_at: now,
                expires_at: now + LOCK_TTL_MS,
            };
            let text = serde_json::to_string_pretty(&info)?;
            if let Err(err) = file
                .write_all(text.as_bytes())
                .and_then(|_| file.sync_all())
            {
                let _ = fs::remove_file(&lock_path);
                return Err(AppError::Io(err));
            }
            Ok(TryOutcome::Acquired(FileLock { path: lock_path }))
        }
        Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => {
            if is_stale(&lock_path) {
                let _ = fs::remove_file(&lock_path);
                tracing::warn!(path = %lock_path.display(), "removed stale file lock");
                return Ok(TryOutcome::StaleRemoved);
            }
            let holder = read_holder(&lock_path).unwrap_or_else(|| "unknown".to_string());
            Ok(TryOutcome::Busy(holder))
        }
        Err(err) => Err(AppError::Io(err)),
    }
}

fn is_stale(lock_path: &Path) -> bool {
    let now = now_ms();
    if let Ok(text) = fs::read_to_string(lock_path) {
        if let Ok(info) = serde_json::from_str::<LockInfo>(&text) {
            return info.expires_at <= now;
        }
    }

    fs::metadata(lock_path)
        .and_then(|metadata| metadata.modified())
        .ok()
        .and_then(|modified| modified.duration_since(std::time::UNIX_EPOCH).ok())
        .map(|age| now.saturating_sub(age.as_millis() as u64) > LOCK_TTL_MS)
        .unwrap_or(false)
}

fn read_holder(lock_path: &Path) -> Option<String> {
    let text = fs::read_to_string(lock_path).ok()?;
    let info = serde_json::from_str::<LockInfo>(&text).ok()?;
    Some(info.holder)
}

pub fn lock_name(target: &Path) -> String {
    let key = target.to_string_lossy().to_lowercase();
    format!("{:016x}", fnv1a(key.as_bytes()))
}

fn fnv1a(bytes: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for byte in bytes {
        hash ^= u64::from(*byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

#[cfg(test)]
mod tests {
    use super::*;

    fn temp_root(name: &str) -> PathBuf {
        let dir = std::env::temp_dir().join(format!(
            "ai-assistant-lock-{name}-{}-{}",
            std::process::id(),
            now_ms()
        ));
        fs::create_dir_all(&dir).expect("create temp dir");
        dir
    }

    #[tokio::test]
    async fn acquires_and_releases_lock() {
        let root = temp_root("release");
        let target = root.join("target.txt");
        let lock_dir = root.join(".locks");
        let lock = FileLock::acquire_with(
            &lock_dir,
            &root,
            &target,
            "sub_agent_3b_1",
            "task_001",
            1,
            Duration::ZERO,
        )
        .await
        .expect("acquire lock");
        assert!(lock.path().exists());
        drop(lock);
        assert!(!lock_dir
            .join(format!("{}.lock.json", lock_name(&target)))
            .exists());
    }

    #[tokio::test]
    async fn blocks_second_writer_until_expired() {
        let root = temp_root("block");
        let target = root.join("target.txt");
        let lock_dir = root.join(".locks");
        let held = FileLock::acquire_with(
            &lock_dir,
            &root,
            &target,
            "sub_agent_3b_1",
            "task_001",
            1,
            Duration::ZERO,
        )
        .await
        .expect("acquire lock");

        let conflict = FileLock::acquire_with(
            &lock_dir,
            &root,
            &target,
            "sub_agent_3b_2",
            "task_001",
            1,
            Duration::ZERO,
        )
        .await;
        assert!(conflict.is_err());
        drop(held);

        let stale_path = lock_dir.join(format!("{}.lock.json", lock_name(&target)));
        fs::write(
            &stale_path,
            serde_json::to_string(&LockInfo {
                file_path: target.display().to_string(),
                holder: "dead-agent".to_string(),
                task_id: "task_old".to_string(),
                acquired_at: 0,
                expires_at: 1,
            })
            .expect("serialize lock"),
        )
        .expect("write stale lock");

        let recovered = FileLock::acquire_with(
            &lock_dir,
            &root,
            &target,
            "sub_agent_3b_2",
            "task_001",
            2,
            Duration::ZERO,
        )
        .await;
        assert!(recovered.is_ok());
    }

    #[test]
    fn lock_name_is_stable_and_case_insensitive() {
        assert_eq!(
            lock_name(Path::new("C:/Project/SRC/main.rs")),
            lock_name(Path::new("c:/project/src/MAIN.RS"))
        );
    }
}
