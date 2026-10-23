use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskSummary {
    pub id: String,
    pub status: String,
    pub message: String,
    pub created_at: u64,
    pub updated_at: u64,
    #[serde(default)]
    pub summary: String,
    #[serde(default)]
    pub subtask_count: usize,
}

pub struct TaskStore {
    workspace: PathBuf,
}

impl TaskStore {
    pub fn new(data_root: &Path) -> Self {
        Self {
            workspace: data_root.join("workspace"),
        }
    }

    pub fn list(&self, limit: usize) -> Vec<TaskSummary> {
        let Ok(entries) = std::fs::read_dir(&self.workspace) else {
            return Vec::new();
        };
        let mut tasks = entries
            .flatten()
            .filter(|entry| entry.file_type().map(|kind| kind.is_dir()).unwrap_or(false))
            .filter_map(|entry| read_summary(&entry.path()))
            .collect::<Vec<_>>();
        tasks.sort_by_key(|task| std::cmp::Reverse(task.updated_at));
        tasks.truncate(limit);
        tasks
    }

    pub fn get(&self, task_id: &str) -> Option<Value> {
        if task_id.is_empty()
            || !task_id
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-'))
        {
            return None;
        }
        let path = self.workspace.join(task_id).join("task.json");
        let text = std::fs::read_to_string(path).ok()?;
        serde_json::from_str(&text).ok()
    }
}

fn read_summary(dir: &Path) -> Option<TaskSummary> {
    let text = std::fs::read_to_string(dir.join("task.json")).ok()?;
    let value: Value = serde_json::from_str(&text).ok()?;
    let id = string_field(&value, &["taskId", "id"]).or_else(|| {
        dir.file_name()
            .map(|name| name.to_string_lossy().to_string())
    })?;
    Some(TaskSummary {
        id,
        status: string_field(&value, &["status"]).unwrap_or_else(|| "unknown".to_string()),
        message: string_field(&value, &["message", "description"]).unwrap_or_default(),
        created_at: number_field(&value, &["createdAt", "created_at"]),
        updated_at: number_field(
            &value,
            &["updatedAt", "updated_at", "createdAt", "created_at"],
        ),
        summary: string_field(&value, &["summary"]).unwrap_or_default(),
        subtask_count: value
            .get("subtasks")
            .and_then(Value::as_array)
            .map(Vec::len)
            .unwrap_or(0),
    })
}

fn string_field(value: &Value, keys: &[&str]) -> Option<String> {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_str))
        .map(str::to_string)
}

fn number_field(value: &Value, keys: &[&str]) -> u64 {
    keys.iter()
        .find_map(|key| value.get(*key).and_then(Value::as_u64))
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::util::now_ms;

    #[test]
    fn lists_task_records_newest_first() {
        let root = std::env::temp_dir().join(format!(
            "ai-assistant-tasks-{}-{}",
            std::process::id(),
            now_ms()
        ));
        let workspace = root.join("workspace");
        for (id, status, updated) in [("task_1", "done", 10), ("task_2", "failed", 20)] {
            let dir = workspace.join(id);
            std::fs::create_dir_all(&dir).unwrap();
            std::fs::write(
                dir.join("task.json"),
                serde_json::json!({
                    "taskId": id,
                    "status": status,
                    "message": "demo",
                    "createdAt": 1,
                    "updatedAt": updated,
                    "subtasks": [{}, {}]
                })
                .to_string(),
            )
            .unwrap();
        }

        let store = TaskStore::new(&root);
        let tasks = store.list(10);
        assert_eq!(tasks.len(), 2);
        assert_eq!(tasks[0].id, "task_2");
        assert_eq!(tasks[0].subtask_count, 2);
        assert!(store.get("task_1").is_some());
        assert!(store.get("../task_1").is_none());
        let _ = std::fs::remove_dir_all(&root);
    }
}
