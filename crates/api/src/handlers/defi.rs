//! DeFi 借贷接口：抵押、借款、还款、提取、清算与查询。
//!
//! 写操作不再直接修改借贷池，而是构造一笔 `TransactionKind::DefiAction` 交易、
//! 签名后提交到交易池，等待 `/chain/mine` 出块后由链推进借贷池状态。
//! 查询操作直接读取链上借贷池快照。

use crate::state::{with_chain, with_chain_mut, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use rustchain_apps::defi::LoanPosition;
use rustchain_core::defi_payload::{DefiAction, DefiPayload};
use rustchain_core::transaction::{Transaction, TransactionKind};
use serde::{Deserialize, Serialize};
use serde_json::json;

/// DeFi 写操作请求载荷。
#[derive(Debug, Deserialize)]
pub(crate) struct DefiActionRequest {
    /// 仓位所有者地址（必须等于交易发送方）。
    pub(crate) owner: String,
    /// 操作数量。
    pub(crate) amount: u64,
    /// 发送方十六进制私钥，用于对交易签名。
    pub(crate) private_key: String,
    /// 发送方十六进制公钥。
    pub(crate) public_key: String,
    /// 可选交易序号，用于同一区块内的交易去重。
    #[serde(default)]
    pub(crate) nonce: Option<u64>,
}

/// DeFi 清算请求载荷。
#[derive(Debug, Deserialize)]
pub(crate) struct DefiLiquidateRequest {
    /// 被清算地址。
    pub(crate) borrower: String,
    /// 偿还债务数量。
    pub(crate) amount: u64,
    /// 清算人十六进制私钥。
    pub(crate) private_key: String,
    /// 清算人十六进制公钥。
    pub(crate) public_key: String,
    /// 可选交易序号。
    #[serde(default)]
    pub(crate) nonce: Option<u64>,
}

/// DeFi 仓位响应。
#[derive(Debug, Serialize)]
pub(crate) struct DefiPositionResponse {
    /// 仓位用户。
    pub(crate) owner: String,
    /// 抵押数量。
    pub(crate) collateral_amount: u64,
    /// 借款数量。
    pub(crate) debt_amount: u64,
    /// 抵押率（bps）。
    pub(crate) collateral_ratio_bps: u64,
}

impl From<LoanPosition> for DefiPositionResponse {
    fn from(position: LoanPosition) -> Self {
        Self {
            owner: position.owner,
            collateral_amount: position.collateral_amount,
            debt_amount: position.debt_amount,
            collateral_ratio_bps: position.collateral_ratio_bps,
        }
    }
}

/// DeFi 池统计响应。
#[derive(Debug, Serialize)]
pub(crate) struct DefiPoolStatsResponse {
    /// 总抵押。
    pub(crate) total_collateral: u64,
    /// 总债务。
    pub(crate) total_debt: u64,
    /// 当前借款年化利率（bps）。
    pub(crate) borrow_rate_bps: u64,
    /// 已开仓位数量。
    pub(crate) position_count: usize,
}

/// 构造并签名一笔 DeFi 交易载荷。
///
/// `to` 固定为借贷池地址，作为链上业务的目标标识；方向与金额由载荷决定。
/// 签名方（`from`）始终是操作发起人：普通动作即仓位本人，清算时为清算人。
fn build_signed_defi_tx(
    action: DefiAction,
    owner: &str,
    amount: u64,
    private_key: &str,
    public_key: &str,
    nonce: u64,
) -> Result<Transaction, String> {
    let payload = DefiPayload::new(action, owner, amount);
    payload.validate().map_err(|error| error.to_string())?;
    let raw = payload.encode().map_err(|error| error.to_string())?;

    // 从公钥派生发送方地址，保证 from 与签名公钥自洽（链侧会再次校验）。
    let sender = rustchain_crypto::wallet::derive_address_from_public_key(public_key)
        .map_err(|error| format!("公钥非法，无法派生发送方地址: {error}"))?;

    let mut tx = Transaction::new_with_kind(
        TransactionKind::DefiAction,
        sender,
        DEFI_POOL_ADDRESS.to_string(),
        amount,
        nonce,
        Some(raw),
    );
    tx.sign_with_private_key(private_key, public_key)
        .map_err(|error| format!("交易签名失败: {error}"))?;
    tx.validate_for_chain()
        .map_err(|error| format!("交易校验失败: {error}"))?;
    Ok(tx)
}

/// 借贷池在链上的逻辑地址。
const DEFI_POOL_ADDRESS: &str = "defi-lending-pool";

/// 把 DeFi 操作提交为链上交易，返回受理结果。
///
/// 校验顺序：owner 非空 → 构造签名交易 → 入交易池。入池会触发链侧的业务
/// 试执行，因此"必然失败"的操作会在这一步就被拒绝，不会进入交易池。
fn submit_defi_action(
    state: &AppState,
    action: DefiAction,
    owner: &str,
    amount: u64,
    private_key: &str,
    public_key: &str,
    nonce: u64,
) -> (StatusCode, Json<serde_json::Value>) {
    if owner.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "owner 不能为空" })),
        );
    }
    if private_key.trim().is_empty() || public_key.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "private_key 和 public_key 不能为空" })),
        );
    }

    let tx = match build_signed_defi_tx(action, owner, amount, private_key, public_key, nonce) {
        Ok(tx) => tx,
        Err(error) => {
            return (
                StatusCode::BAD_REQUEST,
                Json(json!({ "ok": false, "error": error })),
            )
        }
    };
    let tx_id = tx.id.clone();
    let action_name = action.as_str().to_string();

    match with_chain_mut(state, |chain| {
        chain.add_transaction(tx.clone())?;
        Ok(chain.pending_transactions.len())
    }) {
        Ok(pending_tx_count) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "action": action_name,
                "tx_id": tx_id,
                "pending_tx_count": pending_tx_count,
                "message": "交易已进入交易池，调用 /chain/mine 出块后生效"
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// DeFi 抵押接口（提交链上交易）。
pub(crate) async fn defi_deposit_handler(
    State(state): State<AppState>,
    Json(payload): Json<DefiActionRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    submit_defi_action(
        &state,
        DefiAction::DepositCollateral,
        &payload.owner,
        payload.amount,
        &payload.private_key,
        &payload.public_key,
        payload.nonce.unwrap_or(0),
    )
}

/// DeFi 借款接口（提交链上交易）。
pub(crate) async fn defi_borrow_handler(
    State(state): State<AppState>,
    Json(payload): Json<DefiActionRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    submit_defi_action(
        &state,
        DefiAction::Borrow,
        &payload.owner,
        payload.amount,
        &payload.private_key,
        &payload.public_key,
        payload.nonce.unwrap_or(0),
    )
}

/// DeFi 还款接口（提交链上交易）。
pub(crate) async fn defi_repay_handler(
    State(state): State<AppState>,
    Json(payload): Json<DefiActionRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    submit_defi_action(
        &state,
        DefiAction::Repay,
        &payload.owner,
        payload.amount,
        &payload.private_key,
        &payload.public_key,
        payload.nonce.unwrap_or(0),
    )
}

/// DeFi 提取抵押接口（提交链上交易）。
pub(crate) async fn defi_withdraw_handler(
    State(state): State<AppState>,
    Json(payload): Json<DefiActionRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    submit_defi_action(
        &state,
        DefiAction::WithdrawCollateral,
        &payload.owner,
        payload.amount,
        &payload.private_key,
        &payload.public_key,
        payload.nonce.unwrap_or(0),
    )
}

/// DeFi 清算接口（提交链上交易）。
pub(crate) async fn defi_liquidate_handler(
    State(state): State<AppState>,
    Json(payload): Json<DefiLiquidateRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    submit_defi_action(
        &state,
        DefiAction::Liquidate,
        &payload.borrower,
        payload.amount,
        &payload.private_key,
        &payload.public_key,
        payload.nonce.unwrap_or(0),
    )
}

/// DeFi 查询仓位接口（读取链上状态）。
pub(crate) async fn defi_position_handler(
    State(state): State<AppState>,
    Path(owner): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if owner.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({ "ok": false, "error": "owner 不能为空" })),
        );
    }

    match with_chain(&state, |chain| {
        let position = chain
            .lending_pool
            .positions
            .get(&owner)
            .cloned()
            .ok_or_else(|| rustchain_core::error::CoreError::DefiPositionNotFound {
                owner: owner.clone(),
            })?;
        Ok(DefiPositionResponse::from(position))
    }) {
        Ok(position) => (
            StatusCode::OK,
            Json(json!({ "ok": true, "position": position })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// DeFi 池统计接口（读取链上状态）。
pub(crate) async fn defi_stats_handler(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
    match with_chain(&state, |chain| {
        let pool = &chain.lending_pool;
        Ok(DefiPoolStatsResponse {
            total_collateral: pool.total_collateral,
            total_debt: pool.total_debt,
            borrow_rate_bps: pool.current_borrow_rate_bps(),
            position_count: pool.positions.len(),
        })
    }) {
        Ok(stats) => (StatusCode::OK, Json(json!({ "ok": true, "stats": stats }))),
        Err((status, body)) => (status, Json(body)),
    }
}
