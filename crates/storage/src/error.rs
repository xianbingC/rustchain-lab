use thiserror::Error;

/// 存储模块统一错误类型。
#[derive(Debug, Error)]
pub enum StorageError {
    /// RocksDB 操作失败。
    #[error("RocksDB 操作失败: {0}")]
    RocksDb(String),
    /// LevelDB 操作失败。
    #[error("LevelDB 操作失败: {0}")]
    LevelDb(String),
    /// 键值编码或解码失败。
    #[error("数据编码失败: {0}")]
    Codec(String),
    /// 互斥锁被污染。
    #[error("存储锁异常")]
    PoisonedLock,
}

/// 存储模块统一返回类型。
pub type StorageResult<T> = Result<T, StorageError>;
