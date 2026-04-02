use crate::{
    error::{AppError, AppResult},
    AppConfig,
};
use tracing::Level;

/// 初始化统一日志系统。
pub fn init_logging(config: &AppConfig) -> AppResult<()> {
    let max_level = parse_level(&config.log_level)?;

    tracing_subscriber::fmt()
        .with_target(true)
        .with_max_level(max_level)
        .with_thread_names(true)
        .with_ansi(true)
        .try_init()
        .map_err(|error| AppError::Logging(error.to_string()))
}

/// 将字符串日志级别解析为 tracing 的枚举值。
fn parse_level(level: &str) -> AppResult<Level> {
    match level.to_ascii_lowercase().as_str() {
        "trace" => Ok(Level::TRACE),
        "debug" => Ok(Level::DEBUG),
        "info" => Ok(Level::INFO),
        "warn" | "warning" => Ok(Level::WARN),
        "error" => Ok(Level::ERROR),
        other => Err(AppError::Config(format!("不支持的日志级别: {other}"))),
    }
}
