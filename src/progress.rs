use serde::Serialize;

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TaskProgress {
    pub task_id: String,
    pub stage: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub status: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub subtask_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub agent: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub tool: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub path: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub elapsed_ms: Option<u64>,
}

pub trait ProgressSink: Send + Sync {
    fn emit(&self, progress: TaskProgress);
}

pub struct NoopProgress;

impl ProgressSink for NoopProgress {
    fn emit(&self, _progress: TaskProgress) {}
}

pub fn emit(sink: Option<&dyn ProgressSink>, progress: TaskProgress) {
    if let Some(sink) = sink {
        sink.emit(progress);
    }
}
