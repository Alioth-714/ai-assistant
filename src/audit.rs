use crate::util::now_ms;
use serde_json::{json, Value};
use std::fs::{self, OpenOptions};
use std::io::Write;
use std::path::Path;

pub fn append(data_root: &Path, record: Value) {
    let log_path = data_root.join("logs").join("audit.log");
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

pub fn event(
    data_root: &Path,
    event: &str,
    actor: &str,
    action: &str,
    result: &str,
    details: Value,
) {
    let mut record = json!({
        "ts": now_ms(),
        "event": event,
        "actor": actor,
        "action": action,
        "result": result,
    });
    if let (Some(record), Value::Object(details)) = (record.as_object_mut(), details) {
        for (key, value) in details {
            record.insert(key, value);
        }
    }
    append(data_root, record);
}

/// 读取最近的事件（按 `event` 过滤，最新的在前），供界面展示出网记录等。
pub fn recent(data_root: &Path, event: &str, limit: usize) -> Vec<Value> {
    let log_path = data_root.join("logs").join("audit.log");
    let Ok(text) = fs::read_to_string(&log_path) else {
        return Vec::new();
    };
    let mut records: Vec<Value> = text
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .filter(|value| value.get("event").and_then(Value::as_str) == Some(event))
        .collect();
    records.reverse();
    records.truncate(limit.clamp(1, 200));
    records
}
