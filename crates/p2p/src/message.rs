use serde::{Deserialize, Serialize};
use thiserror::Error;

/// 握手数据，用于节点初次连接时交换基础能力。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Handshake {
    /// 节点唯一标识。
    pub node_id: String,
    /// 协议版本号。
    pub protocol_version: String,
    /// 节点监听地址。
    pub listen_addr: String,
    /// 当前本地区块高度。
    pub best_height: u64,
    /// 当前本地区块哈希。
    pub best_hash: String,
}

/// 链状态摘要，用于快速判断是否需要同步。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ChainStatus {
    /// 链标识。
    pub chain_id: String,
    /// 最新区块高度。
    pub best_height: u64,
    /// 最新区块哈希。
    pub best_hash: String,
    /// 当前难度。
    pub difficulty: u32,
    /// 创世区块哈希。
    pub genesis_hash: String,
}

/// 网络消息结构，覆盖握手、状态同步、交易和区块广播。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub enum NetworkMessage {
    /// 心跳请求。
    Ping { nonce: u64, timestamp: i64 },
    /// 心跳响应。
    Pong { nonce: u64, timestamp: i64 },
    /// 握手协商消息。
    Handshake(Handshake),
    /// 广播新交易（序列化后的交易字节）。
    NewTransaction { transaction: Vec<u8> },
    /// 广播新区块（序列化后的区块字节）。
    NewBlock { block: Vec<u8> },
    /// 请求区块区间数据。
    GetBlocks { from_height: u64, limit: u32 },
    /// 响应区块数据。
    Blocks { blocks: Vec<Vec<u8>> },
    /// 请求交易池快照。
    GetMempool,
    /// 返回交易池快照。
    Mempool { transactions: Vec<Vec<u8>> },
    /// 请求链状态。
    GetChainStatus,
    /// 返回链状态。
    ChainStatus(ChainStatus),
}

/// 消息基础校验错误。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum MessageValidationError {
    /// 字段缺失。
    #[error("字段缺失: {0}")]
    MissingField(&'static str),
    /// 字段超出范围。
    #[error("字段不合法: {0}")]
    InvalidField(&'static str),
    /// 二进制载荷为空。
    #[error("消息载荷不能为空: {0}")]
    EmptyPayload(&'static str),
}

impl NetworkMessage {
    /// 返回消息类型名称，便于日志和诊断输出。
    pub fn message_type(&self) -> &'static str {
        match self {
            Self::Ping { .. } => "ping",
            Self::Pong { .. } => "pong",
            Self::Handshake(_) => "handshake",
            Self::NewTransaction { .. } => "new_transaction",
            Self::NewBlock { .. } => "new_block",
            Self::GetBlocks { .. } => "get_blocks",
            Self::Blocks { .. } => "blocks",
            Self::GetMempool => "get_mempool",
            Self::Mempool { .. } => "mempool",
            Self::GetChainStatus => "get_chain_status",
            Self::ChainStatus(_) => "chain_status",
        }
    }

    /// 对网络消息进行轻量结构校验，避免无效消息进入业务流程。
    pub fn validate_basic(&self) -> Result<(), MessageValidationError> {
        match self {
            Self::Ping { .. } | Self::Pong { .. } | Self::GetMempool | Self::GetChainStatus => Ok(()),
            Self::Handshake(handshake) => handshake.validate_basic(),
            Self::NewTransaction { transaction } => {
                if transaction.is_empty() {
                    return Err(MessageValidationError::EmptyPayload("transaction"));
                }
                Ok(())
            }
            Self::NewBlock { block } => {
                if block.is_empty() {
                    return Err(MessageValidationError::EmptyPayload("block"));
                }
                Ok(())
            }
            Self::GetBlocks { limit, .. } => {
                if *limit == 0 || *limit > 200 {
                    return Err(MessageValidationError::InvalidField("limit"));
                }
                Ok(())
            }
            Self::Blocks { blocks } => {
                if blocks.iter().any(|block| block.is_empty()) {
                    return Err(MessageValidationError::EmptyPayload("blocks"));
                }
                Ok(())
            }
            Self::Mempool { transactions } => {
                if transactions.iter().any(|tx| tx.is_empty()) {
                    return Err(MessageValidationError::EmptyPayload("transactions"));
                }
                Ok(())
            }
            Self::ChainStatus(status) => status.validate_basic(),
        }
    }
}

impl Handshake {
    /// 握手结构基础校验。
    pub fn validate_basic(&self) -> Result<(), MessageValidationError> {
        if self.node_id.trim().is_empty() {
            return Err(MessageValidationError::MissingField("node_id"));
        }
        if self.protocol_version.trim().is_empty() {
            return Err(MessageValidationError::MissingField("protocol_version"));
        }
        if self.listen_addr.trim().is_empty() {
            return Err(MessageValidationError::MissingField("listen_addr"));
        }
        if self.best_hash.trim().is_empty() {
            return Err(MessageValidationError::MissingField("best_hash"));
        }
        Ok(())
    }
}

impl ChainStatus {
    /// 链状态结构基础校验。
    pub fn validate_basic(&self) -> Result<(), MessageValidationError> {
        if self.chain_id.trim().is_empty() {
            return Err(MessageValidationError::MissingField("chain_id"));
        }
        if self.best_hash.trim().is_empty() {
            return Err(MessageValidationError::MissingField("best_hash"));
        }
        if self.genesis_hash.trim().is_empty() {
            return Err(MessageValidationError::MissingField("genesis_hash"));
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证握手消息缺少节点 ID 时会被拒绝。
    #[test]
    fn handshake_without_node_id_should_fail() {
        let msg = NetworkMessage::Handshake(Handshake {
            node_id: String::new(),
            protocol_version: "1.0.0".to_string(),
            listen_addr: "127.0.0.1:7000".to_string(),
            best_height: 1,
            best_hash: "abc".to_string(),
        });

        assert_eq!(
            msg.validate_basic(),
            Err(MessageValidationError::MissingField("node_id"))
        );
    }

    /// 验证区块请求消息的 limit 有范围限制。
    #[test]
    fn get_blocks_with_invalid_limit_should_fail() {
        let msg = NetworkMessage::GetBlocks {
            from_height: 10,
            limit: 0,
        };

        assert_eq!(
            msg.validate_basic(),
            Err(MessageValidationError::InvalidField("limit"))
        );
    }

    /// 验证交易广播消息需要非空载荷。
    #[test]
    fn new_transaction_with_empty_payload_should_fail() {
        let msg = NetworkMessage::NewTransaction {
            transaction: Vec::new(),
        };

        assert_eq!(
            msg.validate_basic(),
            Err(MessageValidationError::EmptyPayload("transaction"))
        );
    }

    /// 验证合法的链状态消息可以通过基础校验。
    #[test]
    fn valid_chain_status_should_pass() {
        let msg = NetworkMessage::ChainStatus(ChainStatus {
            chain_id: "rustchain-lab-dev".to_string(),
            best_height: 9,
            best_hash: "0xabc".to_string(),
            difficulty: 2,
            genesis_hash: "0xgenesis".to_string(),
        });

        assert!(msg.validate_basic().is_ok());
        assert_eq!(msg.message_type(), "chain_status");
    }
}
