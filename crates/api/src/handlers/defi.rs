//! DeFi 借贷接口：抵押、借款、还款、提取、清算与查询。

use crate::state::{now_unix_ts, with_pool, with_pool_mut, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use rustchain_apps::defi::DefiError;
use serde::{Deserialize, Serialize};
use serde_json::json;

/// DeFi 请求载荷。
#[derive(Debug, Deserialize)]
pub(crate) struct DefiActionRequest {
    /// 用户地址。
    pub(crate) owner: String,
    /// 操作数量。
    pub(crate) amount: u64,
}

/// DeFi 清算请求载荷。
#[derive(Debug, Deserialize)]
pub(crate) struct DefiLiquidateRequest {
    /// 被清算地址。
    pub(crate) borrower: String,
    /// 偿还债务数量。
    pub(crate) amount: u64,
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

/// DeFi 抵押接口。
pub(crate) async fn defi_deposit_handler(
    State(state): State<AppState>,
    Json(payload): Json<DefiActionRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if payload.owner.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "owner 不能为空"
            })),
        );
    }

    match with_pool_mut(&state, |pool| {
        let position = pool.deposit_collateral(&payload.owner, payload.amount)?;
        Ok(json!({
            "ok": true,
            "position": {
                "owner": position.owner,
                "collateral_amount": position.collateral_amount,
                "debt_amount": position.debt_amount,
                "collateral_ratio_bps": position.collateral_ratio_bps
            }
        }))
    }) {
        Ok(body) => (StatusCode::OK, Json(body)),
        Err((status, body)) => (status, Json(body)),
    }
}

/// DeFi 借款接口。
pub(crate) async fn defi_borrow_handler(
    State(state): State<AppState>,
    Json(payload): Json<DefiActionRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if payload.owner.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "owner 不能为空"
            })),
        );
    }

    let now_ts = now_unix_ts();
    match with_pool_mut(&state, |pool| {
        let position = pool.borrow(&payload.owner, payload.amount, now_ts)?;
        Ok(json!({
            "ok": true,
            "position": {
                "owner": position.owner,
                "collateral_amount": position.collateral_amount,
                "debt_amount": position.debt_amount,
                "collateral_ratio_bps": position.collateral_ratio_bps
            }
        }))
    }) {
        Ok(body) => (StatusCode::OK, Json(body)),
        Err((status, body)) => (status, Json(body)),
    }
}

/// DeFi 还款接口。
pub(crate) async fn defi_repay_handler(
    State(state): State<AppState>,
    Json(payload): Json<DefiActionRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if payload.owner.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "owner 不能为空"
            })),
        );
    }

    let now_ts = now_unix_ts();
    match with_pool_mut(&state, |pool| {
        let repaid = pool.repay(&payload.owner, payload.amount, now_ts)?;
        let position = pool.positions.get(&payload.owner).cloned().ok_or_else(|| {
            DefiError::PositionNotFound {
                owner: payload.owner.clone(),
            }
        })?;

        Ok(json!({
            "ok": true,
            "repaid_amount": repaid,
            "position": {
                "owner": position.owner,
                "collateral_amount": position.collateral_amount,
                "debt_amount": position.debt_amount,
                "collateral_ratio_bps": position.collateral_ratio_bps
            }
        }))
    }) {
        Ok(body) => (StatusCode::OK, Json(body)),
        Err((status, body)) => (status, Json(body)),
    }
}

/// DeFi 提取抵押接口。
pub(crate) async fn defi_withdraw_handler(
    State(state): State<AppState>,
    Json(payload): Json<DefiActionRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if payload.owner.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "owner 不能为空"
            })),
        );
    }

    let now_ts = now_unix_ts();
    match with_pool_mut(&state, |pool| {
        let position = pool.withdraw_collateral(&payload.owner, payload.amount, now_ts)?;
        Ok(json!({
            "ok": true,
            "position": {
                "owner": position.owner,
                "collateral_amount": position.collateral_amount,
                "debt_amount": position.debt_amount,
                "collateral_ratio_bps": position.collateral_ratio_bps
            }
        }))
    }) {
        Ok(body) => (StatusCode::OK, Json(body)),
        Err((status, body)) => (status, Json(body)),
    }
}

/// DeFi 清算接口。
pub(crate) async fn defi_liquidate_handler(
    State(state): State<AppState>,
    Json(payload): Json<DefiLiquidateRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if payload.borrower.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "borrower 不能为空"
            })),
        );
    }

    let now_ts = now_unix_ts();
    match with_pool_mut(&state, |pool| {
        let outcome = pool.liquidate(&payload.borrower, payload.amount, now_ts)?;
        Ok(json!({
            "ok": true,
            "outcome": outcome
        }))
    }) {
        Ok(body) => (StatusCode::OK, Json(body)),
        Err((status, body)) => (status, Json(body)),
    }
}

/// DeFi 查询仓位接口。
pub(crate) async fn defi_position_handler(
    State(state): State<AppState>,
    Path(owner): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if owner.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "owner 不能为空"
            })),
        );
    }

    match with_pool(&state, |pool| {
        let position =
            pool.positions
                .get(&owner)
                .cloned()
                .ok_or_else(|| DefiError::PositionNotFound {
                    owner: owner.clone(),
                })?;

        let response = DefiPositionResponse {
            owner: position.owner,
            collateral_amount: position.collateral_amount,
            debt_amount: position.debt_amount,
            collateral_ratio_bps: position.collateral_ratio_bps,
        };

        Ok(json!({
            "ok": true,
            "position": response
        }))
    }) {
        Ok(body) => (StatusCode::OK, Json(body)),
        Err((status, body)) => (status, Json(body)),
    }
}

/// DeFi 池统计接口。
pub(crate) async fn defi_stats_handler(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
    match with_pool(&state, |pool| {
        let stats = DefiPoolStatsResponse {
            total_collateral: pool.total_collateral,
            total_debt: pool.total_debt,
            borrow_rate_bps: pool.current_borrow_rate_bps(),
            position_count: pool.positions.len(),
        };

        Ok(json!({
            "ok": true,
            "stats": stats
        }))
    }) {
        Ok(body) => (StatusCode::OK, Json(body)),
        Err((status, body)) => (status, Json(body)),
    }
}
