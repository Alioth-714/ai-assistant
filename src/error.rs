use thiserror::Error;

#[derive(Debug, Error)]
pub enum AppError {
    #[error("配置错误：{0}")]
    Config(String),
    #[error("文件操作失败：{0}")]
    Io(#[from] std::io::Error),
    #[error("模型服务不可用：{0}")]
    Ollama(String),
    #[error("外部模型调用失败：{0}")]
    External(String),
    #[error("模型调用超时（{0} 秒）")]
    Timeout(u64),
    #[error("参数错误：{0}")]
    InvalidInput(String),
    #[error("序列化失败：{0}")]
    Serde(#[from] serde_json::Error),
}

pub type Result<T> = std::result::Result<T, AppError>;
