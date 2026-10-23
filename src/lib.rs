pub mod audit;
pub mod config;
pub mod coordinator;
pub mod data;
pub mod environment;
pub mod error;
pub mod file_lock;
pub mod logging;
pub mod memory;
pub mod ollama;
pub mod orchestrator;
pub mod permissions;
pub mod progress;
pub mod roles;
pub mod router;
pub mod runtime;
pub mod subagent;
pub mod tasks;
pub mod tools;
pub mod util;

pub use coordinator::ToolTrace;
pub use environment::EnvironmentReport;
pub use error::{AppError, Result};
pub use memory::{LongTermFact, SessionMessage};
pub use permissions::{
    AutoApprove, AutoDeny, PermissionLevel, PermissionPrompt, PermissionRequest,
};
pub use progress::{ProgressSink, TaskProgress};
pub use router::RouterDecision;
pub use runtime::{bootstrap, AppStatus, ChatReply, Runtime};
pub use tasks::TaskSummary;
