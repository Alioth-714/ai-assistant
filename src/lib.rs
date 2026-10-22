pub mod config;
pub mod coordinator;
pub mod data;
pub mod error;
pub mod logging;
pub mod memory;
pub mod ollama;
pub mod permissions;
pub mod roles;
pub mod router;
pub mod runtime;
pub mod tools;
pub mod util;

pub use coordinator::ToolTrace;
pub use error::{AppError, Result};
pub use memory::SessionMessage;
pub use permissions::{
    AutoApprove, AutoDeny, PermissionLevel, PermissionPrompt, PermissionRequest,
};
pub use router::RouterDecision;
pub use runtime::{bootstrap, AppStatus, ChatReply, Runtime};
