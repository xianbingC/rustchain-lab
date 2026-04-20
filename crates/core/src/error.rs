use thiserror::Error;

/// 区块链核心模块的错误定义。
#[derive(Debug, Error, PartialEq, Eq)]
pub enum CoreError {
    /// 交易金额不能为零。
    #[error("交易金额必须大于 0")]
    ZeroAmount,
    /// 普通交易必须提供发送方地址。
    #[error("普通交易缺少发送方地址")]
    MissingSender,
    /// 外部提交时不允许直接伪造系统地址。
    #[error("不允许通过外部接口提交系统交易")]
    ReservedSystemAddress,
    /// 交易必须提供接收方地址。
    #[error("交易缺少接收方地址")]
    MissingRecipient,
    /// 普通交易必须提供签名。
    #[error("普通交易缺少签名")]
    MissingSignature,
    /// 普通交易必须提供发送者公钥。
    #[error("普通交易缺少发送者公钥")]
    MissingSenderPublicKey,
    /// 发送者地址与发送者公钥不匹配。
    #[error("发送者地址与公钥不匹配")]
    SenderAddressMismatch,
    /// 交易签名无效。
    #[error("交易签名无效")]
    InvalidTransactionSignature,
    /// 交易 ID 与重新计算后的结果不一致。
    #[error("交易 ID 校验失败: {0}")]
    InvalidTransactionId(String),
    /// 区块哈希不正确。
    #[error("区块哈希校验失败: index={index}")]
    InvalidBlockHash { index: u64 },
    /// 前一区块哈希不正确。
    #[error("前一区块哈希不匹配: index={index}")]
    InvalidPreviousHash { index: u64 },
    /// Merkle 根不正确。
    #[error("Merkle Root 校验失败: index={index}")]
    InvalidMerkleRoot { index: u64 },
    /// 工作量证明不满足当前难度要求。
    #[error("工作量证明校验失败: index={index}")]
    InvalidProofOfWork { index: u64 },
    /// 区块难度与链配置不一致。
    #[error("区块难度不匹配: index={index}, expected={expected}, actual={actual}")]
    InvalidBlockDifficulty {
        index: u64,
        expected: u32,
        actual: u32,
    },
    /// 区块索引不连续。
    #[error("区块索引不连续: expected={expected}, actual={actual}")]
    InvalidBlockIndex { expected: u64, actual: u64 },
    /// 账户余额不足。
    #[error("账户余额不足: address={address}, needed={needed}, available={available}")]
    InsufficientBalance {
        address: String,
        needed: u64,
        available: u64,
    },
    /// 链为空，无法继续校验或出块。
    #[error("当前区块链为空")]
    EmptyChain,
    /// 创世区块不符合预期。
    #[error("创世区块内容不合法")]
    InvalidGenesisBlock,
    /// 加密相关处理失败。
    #[error("加密处理失败: {0}")]
    CryptoOperationFailed(String),
    /// 合约载荷不是合法 UTF-8 文本。
    #[error("合约载荷编码非法: tx_id={tx_id}, reason={reason}")]
    ContractPayloadEncodingInvalid { tx_id: String, reason: String },
    /// 合约编译失败。
    #[error("合约编译失败: tx_id={tx_id}, reason={reason}")]
    ContractCompileFailed { tx_id: String, reason: String },
    /// 合约执行失败。
    #[error("合约执行失败: tx_id={tx_id}, reason={reason}")]
    ContractExecutionFailed { tx_id: String, reason: String },
}
