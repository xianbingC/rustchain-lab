use thiserror::Error;

/// 加密模块统一错误类型。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CryptoError {
    /// 十六进制编码解析失败。
    #[error("十六进制编码不合法: {0}")]
    InvalidHex(String),
    /// 密钥长度不合法。
    #[error("密钥长度不合法: expected={expected}, actual={actual}")]
    InvalidKeyLength { expected: usize, actual: usize },
    /// 签名长度不合法。
    #[error("签名长度不合法: expected={expected}, actual={actual}")]
    InvalidSignatureLength { expected: usize, actual: usize },
    /// 签名验证失败。
    #[error("签名验证失败")]
    InvalidSignature,
    /// 密码不能为空。
    #[error("密码不能为空")]
    EmptyPassword,
    /// 钱包解密失败，可能是密码错误或数据损坏。
    #[error("钱包解密失败")]
    WalletDecryptFailed,
}

/// 加密模块统一返回类型。
pub type CryptoResult<T> = Result<T, CryptoError>;
