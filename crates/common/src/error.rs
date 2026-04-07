use thiserror::Error;

/// 全项目可复用的应用层错误类型。
#[derive(Debug, Error)]
pub enum AppError {
    /// 配置项缺失或格式错误。
    #[error("配置错误: {0}")]
    Config(String),
    /// IO 相关错误。
    #[error("IO 错误: {0}")]
    Io(#[from] std::io::Error),
    /// 日志系统初始化失败。
    #[error("日志初始化失败: {0}")]
    Logging(String),
    /// CLI 或应用命令执行失败。
    #[error("命令执行失败: {0}")]
    Command(String),
}

/// 全项目统一的返回结果类型。
pub type AppResult<T> = Result<T, AppError>;
