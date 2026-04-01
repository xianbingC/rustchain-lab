pub mod block;
pub mod blockchain;
pub mod error;
pub mod hash;
pub mod merkle;
pub mod pow;
pub mod transaction;

/// `core` 模块统一返回的结果类型。
pub type CoreResult<T> = Result<T, error::CoreError>;
