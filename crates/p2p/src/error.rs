use thiserror::Error;

/// P2P 模块统一错误类型。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum P2pError {
    /// 消息序列化失败。
    #[error("消息序列化失败: {0}")]
    Serialize(String),
    /// 消息反序列化失败。
    #[error("消息反序列化失败: {0}")]
    Deserialize(String),
    /// 消息结构校验失败。
    #[error("消息结构校验失败: {0}")]
    InvalidMessage(String),
    /// 序号早于当前窗口，直接丢弃。
    #[error("消息序号过旧: seq={seq}, expected={expected}")]
    StaleSequence { seq: u64, expected: u64 },
    /// 参数不合法。
    #[error("参数不合法: {0}")]
    InvalidArgument(String),
}

/// P2P 模块统一返回类型。
pub type P2pResult<T> = Result<T, P2pError>;
