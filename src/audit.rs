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
