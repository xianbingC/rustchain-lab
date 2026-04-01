use crate::{hash::sha256_hex_parts, CoreResult};
use chrono::Utc;
use serde::{Deserialize, Serialize};

/// 系统奖励交易使用的发送方地址。
pub const SYSTEM_ADDRESS: &str = "__system__";

/// 链上交易数据结构。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Transaction {
    /// 交易唯一标识。
    pub id: String,
    /// 发送者地址。
    pub from: String,
    /// 接收者地址。
    pub to: String,
    /// 转账金额。
    pub amount: u64,
    /// 交易创建时间戳。
    pub timestamp: i64,
    /// 交易签名，原型阶段使用十六进制字符串表示。
    pub signature: Option<String>,
    /// 扩展数据字段，可承载合约调用等业务信息。
    pub payload: Option<Vec<u8>>,
}

impl Transaction {
    /// 创建一笔普通交易，并立即生成初始交易 ID。
    pub fn new(from: impl Into<String>, to: impl Into<String>, amount: u64, payload: Option<Vec<u8>>) -> Self {
        let mut tx = Self {
            id: String::new(),
            from: from.into(),
            to: to.into(),
            amount,
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
            from: SYSTEM_ADDRESS.to_string(),
            to: to.into(),
            amount,
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
            "{}|{}|{}|{}|{}",
            self.from, self.to, self.amount, self.timestamp, payload_hex
        )
        .into_bytes()
    }

    /// 基于交易内容和当前签名重新计算交易 ID。
    pub fn calculate_id(&self) -> String {
        let signing_payload = self.signing_payload();
        let signature = self.signature.as_deref().unwrap_or_default();

        sha256_hex_parts(&[
            signing_payload.as_slice(),
            signature.as_bytes(),
        ])
    }

    /// 交易签名更新后，需要同步刷新交易 ID。
    pub fn refresh_id(&mut self) {
        self.id = self.calculate_id();
    }

    /// 判断是否为系统交易。
    pub fn is_system(&self) -> bool {
        self.from == SYSTEM_ADDRESS
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
        assert!(tx.validate_basic().is_ok());
    }
}
