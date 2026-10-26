use crate::error::{AppError, Result};
use std::path::Path;
use tracing_appender::non_blocking::WorkerGuard;
use tracing_appender::rolling::{RollingFileAppender, Rotation};
use tracing_subscriber::layer::SubscriberExt;
use tracing_subscriber::util::SubscriberInitExt;
use tracing_subscriber::EnvFilter;

/// 日志保留份数（按天轮转，保留最近 N 天）
const KEEP_DAYS: usize = 7;

pub fn init(data_root: &Path) -> Result<WorkerGuard> {
    let log_dir = data_root.join("logs");
    std::fs::create_dir_all(&log_dir)?;

    // 生产日志：按天轮转 + 限制保留份数，避免单文件无限增长
    let file_appender = RollingFileAppender::builder()
        .rotation(Rotation::DAILY)
        .filename_prefix("app")
        .filename_suffix("log")
        .max_log_files(KEEP_DAYS)
        .build(&log_dir)
        .map_err(|err| AppError::Config(format!("初始化日志轮转失败：{err}")))?;
    let (file_writer, guard) = tracing_appender::non_blocking(file_appender);

    let filter = EnvFilter::try_from_default_env()
        .unwrap_or_else(|_| EnvFilter::new("ai_assistant=info,ai_assistant_app=info,info"));

    // 仅 debug 构建输出 console（生产不刷屏，日志全在文件里）
    #[cfg(debug_assertions)]
    let registry = tracing_subscriber::registry().with(filter).with(
        tracing_subscriber::fmt::layer()
            .with_writer(std::io::stdout)
            .with_ansi(true),
    );
    #[cfg(not(debug_assertions))]
    let registry = tracing_subscriber::registry().with(filter);

    let _ = registry
        .with(
            tracing_subscriber::fmt::layer()
                .with_writer(file_writer)
                .with_ansi(false),
        )
        .try_init();

    Ok(guard)
}
