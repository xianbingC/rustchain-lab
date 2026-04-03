use crate::{hash::sha256_hex_parts, CoreResult};
use chrono::Utc;
use serde::{Deserialize, Serialize};

/// 系统奖励交易使用的发送方地址。
pub const SYSTEM_ADDRESS: &str = "__system__";

/// 交易类型，用于在不破坏底层结构的前提下区分业务语义。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum TransactionKind {
    /// 普通转账交易。
    Transfer,
    /// 合约部署交易。
    ContractDeploy,
    /// 合约调用交易。
    ContractCall,
    /// DeFi 业务动作交易。
    DefiAction,
    /// NFT 铸造交易。
    NftMint,
    /// NFT 转移交易。
    NftTransfer,
    /// 系统奖励交易。
    SystemReward,
}

impl TransactionKind {
    /// 返回交易类型的稳定字符串表示，用于交易 ID 计算。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Transfer => "transfer",
            Self::ContractDeploy => "contract_deploy",
            Self::ContractCall => "contract_call",
            Self::DefiAction => "defi_action",
            Self::NftMint => "nft_mint",
            Self::NftTransfer => "nft_transfer",
            Self::SystemReward => "system_reward",
        }
    }
}

/// 链上交易数据结构。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    /// 交易唯一标识。
    pub id: String,
    /// 交易类型。
    pub kind: TransactionKind,
    /// 发送者地址。
    pub from: String,
    /// 接收者地址。
    pub to: String,
    /// 转账金额。
    pub amount: u64,
    /// 账户交易序号，用于后续防重放扩展。
    pub nonce: u64,
    /// 交易创建时间戳。
    pub timestamp: i64,
    /// 交易签名，原型阶段使用十六进制字符串表示。
    pub signature: Option<String>,
    /// 扩展数据字段，可承载合约调用等业务信息。
    pub payload: Option<Vec<u8>>,
}

impl Transaction {
    /// 创建一笔普通转账交易，并立即生成初始交易 ID。
    pub fn new(
        from: impl Into<String>,
        to: impl Into<String>,
        amount: u64,
        payload: Option<Vec<u8>>,
    ) -> Self {
        Self::new_with_kind(
            TransactionKind::Transfer,
            from,
            to,
            amount,
            0,
            payload,
        )
    }

    /// 创建自定义类型交易，便于后续合约、DeFi、NFT 场景复用。
    pub fn new_with_kind(
        kind: TransactionKind,
        from: impl Into<String>,
        to: impl Into<String>,
        amount: u64,
        nonce: u64,
        payload: Option<Vec<u8>>,
    ) -> Self {
        let mut tx = Self {
            id: String::new(),
            kind,
            from: from.into(),
            to: to.into(),
            amount,
            nonce,
            timestamp: Utc::now().timestamp(),
            signature: None,
            payload,
        };
        tx.refresh_id();
        tx
    }

    /// 创建系统奖励交易，用于出块奖励或链内系统事件。
    pub fn system(to: impl Into<String>, amount: u64, payload: Option<Vec<u8>>) -> Self {
        let mut tx = Self {
            id: String::new(),
            kind: TransactionKind::SystemReward,
            from: SYSTEM_ADDRESS.to_string(),
            to: to.into(),
            amount,
            nonce: 0,
            timestamp: Utc::now().timestamp(),
            signature: None,
            payload,
        };
        tx.refresh_id();
        tx
    }

    /// 为签名阶段生成稳定的消息摘要输入。
    pub fn signing_payload(&self) -> Vec<u8> {
        let payload_hex = self.payload.as_deref().map(hex::encode).unwrap_or_default();

        format!(
            "{}|{}|{}|{}|{}|{}|{}",
            self.kind.as_str(),
            self.from,
            self.to,
            self.amount,
            self.nonce,
            self.timestamp,
            payload_hex
        )
        .into_bytes()
    }

    /// 基于交易内容和当前签名重新计算交易 ID。
    pub fn calculate_id(&self) -> String {
        let signing_payload = self.signing_payload();
        let signature = self.signature.as_deref().unwrap_or_default();

        sha256_hex_parts(&[signing_payload.as_slice(), signature.as_bytes()])
    }

    /// 交易签名更新后，需要同步刷新交易 ID。
    pub fn refresh_id(&mut self) {
        self.id = self.calculate_id();
    }

    /// 判断是否为系统交易。
    pub fn is_system(&self) -> bool {
        self.kind == TransactionKind::SystemReward || self.from == SYSTEM_ADDRESS
    }

    /// 验证交易的基础结构是否合法。
    pub fn validate_basic(&self) -> CoreResult<()> {
        if self.amount == 0 {
            return Err(crate::error::CoreError::ZeroAmount);
        }

        if !self.is_system() && self.from.trim().is_empty() {
            return Err(crate::error::CoreError::MissingSender);
        }

        if self.to.trim().is_empty() {
            return Err(crate::error::CoreError::MissingRecipient);
        }

        if self.id != self.calculate_id() {
            return Err(crate::error::CoreError::InvalidTransactionId(
                self.id.clone(),
            ));
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证新交易会立即生成合法 ID，并通过基础校验。
    #[test]
    fn new_transaction_should_have_valid_id() {
        let tx = Transaction::new("alice", "bob", 10, Some(vec![1, 2, 3]));

        assert!(!tx.id.is_empty());
        assert_eq!(tx.kind, TransactionKind::Transfer);
        assert_eq!(tx.nonce, 0);
        assert!(tx.validate_basic().is_ok());
    }

    /// 验证交易类型和 nonce 参与交易 ID 计算，避免不同业务被误判为同一交易。
    #[test]
    fn transaction_kind_and_nonce_should_affect_transaction_id() {
        let tx1 = Transaction::new_with_kind(
            TransactionKind::ContractCall,
            "alice",
            "contract-1",
            1,
            7,
            Some(vec![0xAA]),
        );
        let tx2 = Transaction::new_with_kind(
            TransactionKind::ContractCall,
            "alice",
            "contract-1",
            1,
            8,
            Some(vec![0xAA]),
        );

        assert_ne!(tx1.id, tx2.id);
    }
}
