//! DeFi 链上业务载荷。
//!
//! DeFi 操作以 `TransactionKind::DefiAction` 交易的形式上链，业务参数编码在
//! `Transaction.payload` 中。载荷采用 bincode 结构化编码，而非合约那样的 UTF-8
//! 文本源码——DeFi 动作是固定的枚举而非可编程脚本，结构化编码更省空间且不易歧义。
//!
//! 重要约束：`payload` 参与交易签名与交易 ID 计算（见 `Transaction::signing_payload`），
//! 因此本模块的编码格式一旦变更，历史交易 ID 会随之改变，属于破坏性变更。

use serde::{Deserialize, Serialize};
use thiserror::Error;

/// DeFi 动作类型。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DefiAction {
    /// 抵押资产入池。
    DepositCollateral,
    /// 借出资产。
    Borrow,
    /// 偿还债务。
    Repay,
    /// 提取抵押资产。
    WithdrawCollateral,
    /// 清算不健康仓位。
    Liquidate,
}

impl DefiAction {
    /// 返回动作的稳定字符串表示，用于日志与接口输出。
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::DepositCollateral => "deposit_collateral",
            Self::Borrow => "borrow",
            Self::Repay => "repay",
            Self::WithdrawCollateral => "withdraw_collateral",
            Self::Liquidate => "liquidate",
        }
    }
}

/// DeFi 链上载荷。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DefiPayload {
    /// 业务动作。
    pub action: DefiAction,
    /// 仓位主体地址。
    ///
    /// 抵押/借款/还款/提取时表示仓位所有者；清算时表示被清算的借款人。
    /// 该地址必须等于交易的发送方——清算除外，见 [`Self::is_self_operated`]。
    pub owner: String,
    /// 操作数量。
    pub amount: u64,
}

impl DefiPayload {
    /// 构造载荷。
    pub fn new(action: DefiAction, owner: impl Into<String>, amount: u64) -> Self {
        Self {
            action,
            owner: owner.into(),
            amount,
        }
    }

    /// 判断该动作是否必须由仓位本人发起。
    ///
    /// 清算由第三方（清算人）对被清算人的仓位发起，因此 `owner`（借款人）
    /// 允许与交易发送方不同；其余动作都只能操作自己的仓位，防止代他人操作。
    pub fn is_self_operated(&self) -> bool {
        !matches!(self.action, DefiAction::Liquidate)
    }

    /// 将载荷编码为交易 payload 字节。
    pub fn encode(&self) -> Result<Vec<u8>, DefiPayloadError> {
        bincode::serialize(self).map_err(|error| DefiPayloadError::Encode(error.to_string()))
    }

    /// 从交易 payload 字节解码。
    pub fn decode(raw: &[u8]) -> Result<Self, DefiPayloadError> {
        bincode::deserialize(raw).map_err(|error| DefiPayloadError::Decode(error.to_string()))
    }

    /// 校验载荷字段合法性与动作/金额约束。
    pub fn validate(&self) -> Result<(), DefiPayloadError> {
        if self.owner.trim().is_empty() {
            return Err(DefiPayloadError::EmptyOwner);
        }
        if self.amount == 0 {
            return Err(DefiPayloadError::ZeroAmount);
        }

        // 清算动作没有额外约束，其余动作均要求金额为正（已在上方校验）。
        Ok(())
    }
}

/// DeFi 载荷编解码与校验错误。
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum DefiPayloadError {
    /// 载荷编码失败。
    #[error("DeFi 载荷编码失败: {0}")]
    Encode(String),
    /// 载荷解码失败。
    #[error("DeFi 载荷解码失败: {0}")]
    Decode(String),
    /// 仓位所有者地址为空。
    #[error("DeFi 载荷 owner 不能为空")]
    EmptyOwner,
    /// 操作金额必须大于 0。
    #[error("DeFi 载荷 amount 必须大于 0")]
    ZeroAmount,
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证载荷可以完成编解码闭环。
    #[test]
    fn defi_payload_should_roundtrip() {
        let payload = DefiPayload::new(DefiAction::Borrow, "alice", 120);
        let raw = payload.encode().expect("编码应成功");
        let decoded = DefiPayload::decode(&raw).expect("解码应成功");

        assert_eq!(decoded, payload);
        assert_eq!(decoded.action.as_str(), "borrow");
    }

    /// 验证空 owner 会被拒绝。
    #[test]
    fn defi_payload_with_empty_owner_should_fail_validation() {
        let payload = DefiPayload::new(DefiAction::Borrow, "  ", 10);

        assert_eq!(payload.validate(), Err(DefiPayloadError::EmptyOwner));
    }

    /// 验证金额为 0 会被拒绝。
    #[test]
    fn defi_payload_with_zero_amount_should_fail_validation() {
        let payload = DefiPayload::new(DefiAction::Repay, "alice", 0);

        assert_eq!(payload.validate(), Err(DefiPayloadError::ZeroAmount));
    }

    /// 验证非法字节会解码失败而不是 panic。
    #[test]
    fn defi_payload_decode_garbage_should_fail() {
        let result = DefiPayload::decode(&[0xFF, 0x00, 0x13, 0x37]);

        assert!(matches!(result, Err(DefiPayloadError::Decode(_))));
    }
}
