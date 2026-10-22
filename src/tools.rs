use crate::error::{AppError, Result};
use crate::permissions::{confirm_write, PermissionLevel, PermissionPrompt, PermissionRequest};
use crate::util::now_ms;
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::Instant;

const MAX_READ_CHARS: usize = 8_000;
static TEMP_SEQ: AtomicU64 = AtomicU64::new(1);

struct Outcome {
    payload: Value,
    path: Option<String>,
    backup: Option<String>,
}

pub struct ToolExecution {
    pub name: String,
    pub ok: bool,
    pub path: Option<String>,
    pub backup: Option<String>,
    pub elapsed_ms: u64,
    pub payload: Value,
}

impl ToolExecution {
    pub fn content(&self) -> String {
        self.payload.to_string()
    }
}

pub struct ToolExecutor<'a> {
    app_root: PathBuf,
    data_root: PathBuf,
    prompt: Option<&'a dyn PermissionPrompt>,
}

impl<'a> ToolExecutor<'a> {
    pub fn new(
        app_root: &Path,
        data_root: &Path,
        prompt: Option<&'a dyn PermissionPrompt>,
    ) -> Self {
        Self {
            app_root: app_root
                .canonicalize()
                .unwrap_or_else(|_| app_root.to_path_buf()),
            data_root: data_root
                .canonicalize()
                .unwrap_or_else(|_| data_root.to_path_buf()),
            prompt,
        }
    }

    pub async fn execute(&self, name: &str, args: Value) -> ToolExecution {
        let started = Instant::now();
        let raw_path = args
            .get("path")
            .and_then(Value::as_str)
            .map(ToString::to_string);
        let level = match name {
            "write_file" => PermissionLevel::L2,
            _ => PermissionLevel::L0,
        };

        let result = match name {
            "read_file" => self.read_file(&args),
            "write_file" => self.write_file(&args).await,
            _ => Err(AppError::InvalidInput(format!("未知工具：{name}"))),
        };

        let (ok, payload, path, backup) = match result {
            Ok(outcome) => (
                true,
                outcome.payload,
                outcome.path.or(raw_path),
                outcome.backup,
            ),
            Err(err) => (
                false,
                json!({"ok": false, "error": err.to_string()}),
                raw_path,
                None,
            ),
        };
        let elapsed_ms = started.elapsed().as_millis() as u64;
        let error = payload.get("error").and_then(Value::as_str);
        self.audit(name, level, path.as_deref(), ok, backup.as_deref(), error);

        tracing::info!(
            tool = name,
            ok,
            path = path.as_deref().unwrap_or("-"),
            backup = backup.as_deref().unwrap_or("-"),
            elapsed_ms,
            "tool executed"
        );

        ToolExecution {
            name: name.to_string(),
            ok,
            path,
            backup,
            elapsed_ms,
            payload,
        }
    }

    fn read_file(&self, args: &Value) -> Result<Outcome> {
        let raw = required_path(args)?;
        let path = self.resolve_existing(&raw)?;
        ensure_not_sensitive(&path)?;

        if path.is_dir() {
            return Err(AppError::InvalidInput(format!(
                "目标是目录，不是文件：{}",
                self.display_path(&path)
            )));
        }

        let content = fs::read_to_string(&path).map_err(|err| {
            AppError::Io(std::io::Error::new(
                err.kind(),
                format!("读取 {} 失败：{err}", self.display_path(&path)),
            ))
        })?;
        let total_chars = content.chars().count();
        let truncated = total_chars > MAX_READ_CHARS;
        let content: String = content.chars().take(MAX_READ_CHARS).collect();
        let display = self.display_path(&path);

        Ok(Outcome {
            payload: json!({
                "ok": true,
                "path": display,
                "chars": content.chars().count(),
                "totalChars": total_chars,
                "truncated": truncated,
                "content": content
            }),
            path: Some(display),
            backup: None,
        })
    }

    async fn write_file(&self, args: &Value) -> Result<Outcome> {
        let raw = required_path(args)?;
        let path = self.resolve_for_write(&raw)?;
        ensure_not_sensitive(&path)?;

        if path.is_dir() {
            return Err(AppError::InvalidInput(format!(
                "目标是目录，不是文件：{}",
                self.display_path(&path)
            )));
        }

        let old_content = if path.exists() {
            Some(fs::read_to_string(&path).map_err(|err| {
                AppError::Io(std::io::Error::new(
                    err.kind(),
                    format!("读取 {} 失败：{err}", self.display_path(&path)),
                ))
            })?)
        } else {
            None
        };

        let line = args.get("line").and_then(as_u64_lenient);
        let text = args.get("text").and_then(Value::as_str);
        let content = args.get("content").and_then(Value::as_str);

        let (new_content, mode, line_number, old_line) = if let Some(line) = line {
            let text = text.ok_or_else(|| {
                AppError::InvalidInput("使用 line 修改单行时，必须同时提供 text".to_string())
            })?;
            let line_number = line as usize;
            let old = old_content
                .as_deref()
                .ok_or_else(|| AppError::InvalidInput("文件不存在，无法按行修改".to_string()))?;
            let old_line = line_at(old, line_number)?;
            (
                replace_line(old, line_number, text)?,
                "line",
                Some(line_number),
                Some(old_line),
            )
        } else if let Some(content) = content {
            (content.to_string(), "content", None, None)
        } else {
            return Err(AppError::InvalidInput(
                "write_file 需要提供 content（整体覆盖），或 line + text（替换单行）".to_string(),
            ));
        };

        let display = self.display_path(&path);
        let preview = match mode {
            "line" => format!(
                "第 {} 行\n- {}\n+ {}",
                line_number.unwrap_or_default(),
                truncate(old_line.as_deref().unwrap_or(""), 200),
                truncate(text.unwrap_or_default(), 200)
            ),
            _ => format!(
                "整体覆盖\n{} 字符 -> {} 字符",
                old_content
                    .as_deref()
                    .map(|text| text.chars().count())
                    .unwrap_or(0),
                new_content.chars().count()
            ),
        };

        let request = PermissionRequest::write(display.clone(), preview);
        if !confirm_write(self.prompt, request).await {
            return Err(AppError::InvalidInput(
                "写入需要用户确认，但确认被拒绝或等待超时".to_string(),
            ));
        }

        let backup = if old_content.is_some() {
            Some(self.backup_file(&path)?)
        } else {
            None
        };

        if let Err(err) = write_atomic(&path, new_content.as_bytes()) {
            if let Some(backup) = &backup {
                let _ = fs::copy(backup, &path);
            }
            return Err(AppError::Io(std::io::Error::new(
                err.kind(),
                format!("写入 {} 失败：{err}", display),
            )));
        }

        let written = fs::read_to_string(&path)?;
        if written != new_content {
            if let Some(backup) = &backup {
                let _ = fs::copy(backup, &path);
            }
            return Err(AppError::Io(std::io::Error::other(format!(
                "写入校验失败，已尝试从备份恢复：{display}"
            ))));
        }

        let backup_display = backup.as_deref().map(|path| self.display_path(path));
        let line_count = written.split_inclusive('\n').count();

        Ok(Outcome {
            payload: json!({
                "ok": true,
                "path": display,
                "mode": mode,
                "line": line_number,
                "lines": line_count,
                "chars": written.chars().count(),
                "backupPath": backup_display
            }),
            path: Some(display),
            backup: backup_display,
        })
    }

    fn resolve_existing(&self, raw: &str) -> Result<PathBuf> {
        let candidate = self.candidate(raw);
        let canonical = candidate.canonicalize().map_err(|err| {
            AppError::InvalidInput(format!("路径不存在或不可访问：{raw}（{err}）"))
        })?;
        self.ensure_allowed(&canonical)?;
        Ok(canonical)
    }

    fn resolve_for_write(&self, raw: &str) -> Result<PathBuf> {
        let candidate = self.candidate(raw);
        if candidate.exists() {
            let canonical = candidate
                .canonicalize()
                .map_err(|err| AppError::InvalidInput(format!("路径不可访问：{raw}（{err}）")))?;
            self.ensure_allowed(&canonical)?;
            return Ok(canonical);
        }

        let parent = candidate
            .parent()
            .ok_or_else(|| AppError::InvalidInput(format!("无效路径：{raw}")))?;
        let parent = parent
            .canonicalize()
            .map_err(|err| AppError::InvalidInput(format!("父目录不可访问：{raw}（{err}）")))?;
        self.ensure_allowed(&parent)?;
        let name = candidate
            .file_name()
            .ok_or_else(|| AppError::InvalidInput(format!("无效文件名：{raw}")))?;
        let path = parent.join(name);
        self.ensure_allowed(&path)?;
        Ok(path)
    }

    fn candidate(&self, raw: &str) -> PathBuf {
        let path = PathBuf::from(raw);
        if path.is_absolute() {
            path
        } else {
            self.app_root.join(path)
        }
    }

    fn ensure_allowed(&self, path: &Path) -> Result<()> {
        if path.starts_with(&self.app_root) || path.starts_with(&self.data_root) {
            Ok(())
        } else {
            Err(AppError::InvalidInput(format!(
                "路径越界，只允许访问项目目录或数据目录：{}",
                path.display()
            )))
        }
    }

    fn backup_file(&self, path: &Path) -> Result<PathBuf> {
        let relative = path
            .strip_prefix(&self.app_root)
            .or_else(|_| path.strip_prefix(&self.data_root))
            .map(Path::to_path_buf)
            .unwrap_or_else(|_| {
                PathBuf::from(
                    path.file_name()
                        .unwrap_or_default()
                        .to_string_lossy()
                        .to_string(),
                )
            });
        let batch = self.data_root.join("backups").join(now_ms().to_string());
        let file_name = relative
            .file_name()
            .map(|name| name.to_string_lossy().to_string())
            .unwrap_or_else(|| "file".to_string());
        let backup_path = batch
            .join(relative)
            .with_file_name(format!("{file_name}.bak"));

        if let Some(parent) = backup_path.parent() {
            fs::create_dir_all(parent)?;
        }
        fs::copy(path, &backup_path)?;
        Ok(backup_path)
    }

    fn display_path(&self, path: &Path) -> String {
        if let Ok(relative) = path.strip_prefix(&self.app_root) {
            return relative.to_string_lossy().replace('\\', "/");
        }
        if let Ok(relative) = path.strip_prefix(&self.data_root) {
            return format!("data/{}", relative.to_string_lossy().replace('\\', "/"));
        }
        path.display().to_string()
    }

    fn audit(
        &self,
        tool: &str,
        level: PermissionLevel,
        path: Option<&str>,
        ok: bool,
        backup: Option<&str>,
        error: Option<&str>,
    ) {
        let record = json!({
            "ts": now_ms(),
            "tool": tool,
            "level": level.as_str(),
            "path": path,
            "result": if ok { "success" } else { "error" },
            "backup_path": backup,
            "error": error
        });
        let log_path = self.data_root.join("logs").join("audit.log");
        if let Some(parent) = log_path.parent() {
            let _ = fs::create_dir_all(parent);
        }
        match OpenOptions::new().create(true).append(true).open(&log_path) {
            Ok(mut file) => {
                if let Err(err) = writeln!(file, "{record}") {
                    tracing::warn!(error = %err, "audit log write failed");
                }
            }
            Err(err) => tracing::warn!(error = %err, "audit log open failed"),
        }
    }
}

fn as_u64_lenient(value: &Value) -> Option<u64> {
    value
        .as_u64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
}

fn required_path(args: &Value) -> Result<String> {
    args.get("path")
        .and_then(Value::as_str)
        .map(|value| value.trim().to_string())
        .filter(|value| !value.is_empty())
        .ok_or_else(|| AppError::InvalidInput("缺少 path 参数".to_string()))
}

fn ensure_not_sensitive(path: &Path) -> Result<()> {
    let name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_lowercase())
        .unwrap_or_default();
    let extension = path
        .extension()
        .map(|ext| ext.to_string_lossy().to_lowercase())
        .unwrap_or_default();

    let blocked_name = name == ".env"
        || name.starts_with(".env.")
        || name == "id_rsa"
        || name == "id_ed25519"
        || name.ends_with(".key")
        || name.ends_with(".pem");
    let blocked_extension = matches!(extension.as_str(), "key" | "pem" | "pfx" | "p12");
    let blocked_dir = path.components().any(|component| {
        let value = component.as_os_str().to_string_lossy().to_lowercase();
        value == ".ssh" || value == ".aws"
    });

    if blocked_name || blocked_extension || blocked_dir {
        return Err(AppError::InvalidInput(format!(
            "拒绝访问敏感文件：{}",
            path.display()
        )));
    }
    Ok(())
}

fn line_at(content: &str, line_number: usize) -> Result<String> {
    let lines: Vec<&str> = content.split_inclusive('\n').collect();
    if line_number == 0 || line_number > lines.len() {
        return Err(AppError::InvalidInput(format!(
            "行号 {line_number} 超出范围 1..{}",
            lines.len()
        )));
    }
    Ok(lines[line_number - 1]
        .trim_end_matches(['\r', '\n'])
        .to_string())
}

fn replace_line(content: &str, line_number: usize, text: &str) -> Result<String> {
    let mut parts: Vec<String> = content
        .split_inclusive('\n')
        .map(ToString::to_string)
        .collect();
    if line_number == 0 || line_number > parts.len() {
        return Err(AppError::InvalidInput(format!(
            "行号 {line_number} 超出范围 1..{}",
            parts.len()
        )));
    }

    let old = &parts[line_number - 1];
    let ending = if old.ends_with("\r\n") {
        "\r\n"
    } else if old.ends_with('\n') {
        "\n"
    } else {
        ""
    };
    let text = text.trim_end_matches(['\r', '\n']);
    parts[line_number - 1] = format!("{text}{ending}");
    Ok(parts.concat())
}

fn write_atomic(path: &Path, bytes: &[u8]) -> std::io::Result<()> {
    let file_name = path
        .file_name()
        .map(|name| name.to_string_lossy().to_string())
        .unwrap_or_else(|| "file".to_string());
    let temp = path.with_file_name(format!(
        ".{file_name}.tmp-{}-{}",
        std::process::id(),
        TEMP_SEQ.fetch_add(1, Ordering::Relaxed)
    ));

    fs::write(&temp, bytes)?;
    if path.exists() {
        if let Err(err) = fs::remove_file(path) {
            let _ = fs::remove_file(&temp);
            return Err(err);
        }
    }
    if let Err(err) = fs::rename(&temp, path) {
        let _ = fs::remove_file(&temp);
        return Err(err);
    }
    Ok(())
}

fn truncate(text: &str, max_chars: usize) -> String {
    text.chars().take(max_chars).collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn replaces_requested_line_and_keeps_newline() {
        let updated = replace_line("one\ntwo\nthree\n", 2, "TWO").unwrap();
        assert_eq!(updated, "one\nTWO\nthree\n");
    }

    #[test]
    fn rejects_out_of_range_line() {
        assert!(replace_line("one\n", 2, "x").is_err());
        assert!(line_at("one\n", 0).is_err());
    }

    #[test]
    fn reads_line_number_from_string() {
        assert_eq!(as_u64_lenient(&json!("2")), Some(2));
        assert_eq!(as_u64_lenient(&json!(2)), Some(2));
    }

    #[test]
    fn blocks_sensitive_files() {
        assert!(ensure_not_sensitive(Path::new(".env")).is_err());
        assert!(ensure_not_sensitive(Path::new("keys/server.pem")).is_err());
        assert!(ensure_not_sensitive(Path::new("src/main.rs")).is_ok());
    }
}
