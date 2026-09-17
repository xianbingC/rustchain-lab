//! 链查询与链操作接口：链信息、区块、交易池、地址统计、合约状态、提交与挖矿。

use crate::chain_ops::{
    decode_i64_from_le_bytes, persist_mined_block, persist_runtime_state, CONTRACT_EVENTS_FIELD,
    CONTRACT_SNAPSHOT_FIELD,
};
use crate::state::{
    chain_status_from_blockchain, with_chain, with_chain_mut, with_history, with_p2p, with_p2p_mut,
    with_state_store, AppState,
};
use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    Json,
};
use rustchain_core::transaction::Transaction;
use rustchain_p2p::message::NetworkMessage;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::collections::HashMap;

/// 提交链交易请求。
#[derive(Debug, Deserialize)]
pub(crate) struct ChainSubmitTxRequest {
    /// 已签名的交易。
    pub(crate) transaction: Transaction,
}

/// 挖矿请求。
#[derive(Debug, Deserialize)]
pub(crate) struct ChainMineRequest {
    /// 矿工地址。
    pub(crate) miner_address: String,
}

/// 链信息响应。
#[derive(Debug, Serialize)]
pub(crate) struct ChainInfoResponse {
    /// 链标识。
    pub(crate) chain_id: String,
    /// 当前高度。
    pub(crate) height: u64,
    /// 最新区块哈希。
    pub(crate) latest_hash: String,
    /// 下一块期望难度。
    pub(crate) difficulty: u32,
    /// 下一块期望难度（显式字段，便于接口自解释）。
    pub(crate) next_block_expected_difficulty: u32,
    /// 最新区块记录的难度。
    pub(crate) latest_block_difficulty: u32,
    /// 目标出块时间（秒）。
    pub(crate) target_block_time_secs: u64,
    /// 难度调整窗口。
    pub(crate) difficulty_adjustment_interval: u64,
    /// 待打包交易数。
    pub(crate) pending_tx_count: usize,
    /// 已知节点数。
    pub(crate) peer_count: usize,
}

/// 链难度响应。
#[derive(Debug, Serialize)]
pub(crate) struct ChainDifficultyResponse {
    /// 当前高度。
    pub(crate) height: u64,
    /// 最新区块记录的难度。
    pub(crate) latest_block_difficulty: u32,
    /// 下一块期望难度。
    pub(crate) next_block_expected_difficulty: u32,
    /// 目标出块时间（秒）。
    pub(crate) target_block_time_secs: u64,
    /// 难度调整窗口。
    pub(crate) difficulty_adjustment_interval: u64,
}

/// 交易池查询参数。
#[derive(Debug, Deserialize)]
pub(crate) struct ChainMempoolQuery {
    /// 返回数量上限，缺省表示不限制。
    pub(crate) limit: Option<usize>,
    /// 分页偏移量。
    pub(crate) offset: Option<usize>,
    /// 按地址过滤（from/to 任一匹配）。
    pub(crate) address: Option<String>,
}

/// 区块列表查询参数。
#[derive(Debug, Deserialize)]
pub(crate) struct ChainBlocksQuery {
    /// 起始高度。
    pub(crate) from_height: Option<u64>,
    /// 返回数量上限。
    pub(crate) limit: Option<usize>,
}

/// 地址交易查询参数。
#[derive(Debug, Deserialize)]
pub(crate) struct ChainAddressTxsQuery {
    /// 返回数量上限。
    pub(crate) limit: Option<usize>,
    /// 方向过滤：all/in/out。
    pub(crate) direction: Option<String>,
    /// 分页偏移量。
    pub(crate) offset: Option<usize>,
}

/// 已确认地址交易记录。
#[derive(Debug, Serialize)]
pub(crate) struct ChainAddressTxRecord {
    /// 所在区块高度。
    pub(crate) block_index: u64,
    /// 所在区块哈希。
    pub(crate) block_hash: String,
    /// 交易相对该地址的方向。
    pub(crate) direction: String,
    /// 交易内容。
    pub(crate) transaction: Transaction,
}

/// 待打包地址交易记录。
#[derive(Debug, Serialize)]
pub(crate) struct ChainAddressPendingTxRecord {
    /// 交易相对该地址的方向。
    pub(crate) direction: String,
    /// 交易内容。
    pub(crate) transaction: Transaction,
}

/// 链信息查询接口。
pub(crate) async fn chain_info_handler(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
    match with_chain(&state, |chain| {
        let latest = chain.latest_block()?;
        let next_difficulty = chain.next_block_expected_difficulty()?;
        let info = ChainInfoResponse {
            chain_id: chain.chain_id.clone(),
            height: latest.index,
            latest_hash: latest.hash.clone(),
            difficulty: next_difficulty,
            next_block_expected_difficulty: next_difficulty,
            latest_block_difficulty: latest.difficulty,
            target_block_time_secs: chain.target_block_time_secs,
            difficulty_adjustment_interval: chain.difficulty_adjustment_interval,
            pending_tx_count: chain.pending_transactions.len(),
            peer_count: chain.peers.len(),
        };
        Ok(json!({
            "ok": true,
            "chain": info
        }))
    }) {
        Ok(body) => (StatusCode::OK, Json(body)),
        Err((status, body)) => (status, Json(body)),
    }
}

/// 链难度详情查询接口。
pub(crate) async fn chain_difficulty_handler(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
    match with_chain(&state, |chain| {
        let latest = chain.latest_block()?;
        let info = ChainDifficultyResponse {
            height: latest.index,
            latest_block_difficulty: latest.difficulty,
            next_block_expected_difficulty: chain.next_block_expected_difficulty()?,
            target_block_time_secs: chain.target_block_time_secs,
            difficulty_adjustment_interval: chain.difficulty_adjustment_interval,
        };
        Ok(json!({
            "ok": true,
            "difficulty": info
        }))
    }) {
        Ok(body) => (StatusCode::OK, Json(body)),
        Err((status, body)) => (status, Json(body)),
    }
}

/// 按高度查询区块接口。
pub(crate) async fn chain_block_by_height_handler(
    State(state): State<AppState>,
    Path(height): Path<u64>,
) -> (StatusCode, Json<serde_json::Value>) {
    match with_chain(&state, |chain| {
        Ok(chain
            .chain
            .iter()
            .find(|block| block.index == height)
            .cloned())
    }) {
        Ok(Some(block)) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "block": block
            })),
        ),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "ok": false,
                "error": format!("高度为 {height} 的区块不存在")
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// 查询最新区块详情接口。
pub(crate) async fn chain_latest_block_handler(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
    match with_chain(&state, |chain| Ok(chain.latest_block()?.clone())) {
        Ok(block) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "block": block
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// 链完整性校验接口。
pub(crate) async fn chain_validate_handler(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
    match with_chain(&state, |chain| {
        chain.validate_chain()?;
        Ok(())
    }) {
        Ok(()) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "valid": true
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// 按区间查询区块列表接口。
pub(crate) async fn chain_blocks_handler(
    State(state): State<AppState>,
    Query(query): Query<ChainBlocksQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    let from_height = query.from_height.unwrap_or(0);
    let limit = query.limit.unwrap_or(20);
    if limit == 0 || limit > 200 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "limit 必须在 1~200 之间"
            })),
        );
    }

    match with_chain(&state, |chain| {
        let latest = chain.latest_block()?.index;
        let blocks = chain
            .chain
            .iter()
            .filter(|block| block.index >= from_height)
            .take(limit)
            .cloned()
            .collect::<Vec<_>>();
        Ok((latest, blocks))
    }) {
        Ok((latest_height, blocks)) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "from_height": from_height,
                "limit": limit,
                "latest_height": latest_height,
                "returned_count": blocks.len(),
                "blocks": blocks
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// 交易池查询接口。
pub(crate) async fn chain_mempool_handler(
    State(state): State<AppState>,
    Query(query): Query<ChainMempoolQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    if matches!(query.limit, Some(0)) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "limit 必须大于 0"
            })),
        );
    }
    if matches!(query.address.as_ref(), Some(address) if address.trim().is_empty()) {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "address 不能为空"
            })),
        );
    }
    let limit = query.limit;
    let offset = query.offset.unwrap_or(0);
    let filter_address = query.address.clone();
    let normalized_filter_address = filter_address
        .as_ref()
        .map(|address| address.trim().to_string());

    match with_chain(&state, |chain| {
        let total = chain.pending_transactions.len();
        let filtered = chain
            .pending_transactions
            .iter()
            .filter(|tx| match normalized_filter_address.as_deref() {
                Some(address) => tx.from == address || tx.to == address,
                None => true,
            })
            .cloned()
            .collect::<Vec<_>>();
        let matched_count = filtered.len();
        let transactions: Vec<Transaction> = match limit {
            Some(limit) => filtered
                .into_iter()
                .skip(offset)
                .take(limit)
                .collect::<Vec<_>>(),
            None => filtered.into_iter().skip(offset).collect::<Vec<_>>(),
        };
        Ok((total, matched_count, transactions))
    }) {
        Ok((total, matched_count, transactions)) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "total_pending_tx_count": total,
                "matched_count": matched_count,
                "filter_address": filter_address,
                "offset": offset,
                "returned_count": transactions.len(),
                "transactions": transactions
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// 按交易 ID 查询待打包交易详情。
pub(crate) async fn chain_pending_tx_handler(
    State(state): State<AppState>,
    Path(tx_id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if tx_id.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "tx_id 不能为空"
            })),
        );
    }

    match with_chain(&state, |chain| {
        Ok(chain
            .pending_transactions
            .iter()
            .find(|tx| tx.id == tx_id)
            .cloned())
    }) {
        Ok(Some(transaction)) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "transaction": transaction
            })),
        ),
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "ok": false,
                "error": format!("待打包交易不存在: {tx_id}")
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// 按地址查询已确认交易列表（from/to 任一匹配）。
pub(crate) async fn chain_address_txs_handler(
    State(state): State<AppState>,
    Path(address): Path<String>,
    Query(query): Query<ChainAddressTxsQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    if address.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "address 不能为空"
            })),
        );
    }

    let limit = query.limit.unwrap_or(20);
    if limit == 0 || limit > 200 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "limit 必须在 1~200 之间"
            })),
        );
    }
    let direction = query
        .direction
        .as_deref()
        .unwrap_or("all")
        .trim()
        .to_ascii_lowercase();
    let offset = query.offset.unwrap_or(0);
    if direction != "all" && direction != "in" && direction != "out" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "direction 必须是 all/in/out"
            })),
        );
    }

    match with_chain(&state, |chain| {
        let mut records = Vec::new();
        for block in chain.chain.iter().rev() {
            for tx in &block.transactions {
                let is_out = tx.from == address;
                let is_in = tx.to == address;
                if !is_out && !is_in {
                    continue;
                }

                if direction == "in" && !is_in {
                    continue;
                }
                if direction == "out" && !is_out {
                    continue;
                }

                let tx_direction = if is_in && is_out {
                    "self".to_string()
                } else if is_out {
                    "out".to_string()
                } else {
                    "in".to_string()
                };

                if direction == "all"
                    || direction == tx_direction
                    || (tx_direction == "self" && (direction == "in" || direction == "out"))
                {
                    records.push(ChainAddressTxRecord {
                        block_index: block.index,
                        block_hash: block.hash.clone(),
                        direction: tx_direction,
                        transaction: tx.clone(),
                    });
                }
            }
        }
        let total = records.len();
        let returned = records
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect::<Vec<_>>();
        Ok((total, returned))
    }) {
        Ok((total, transactions)) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "address": address,
                "direction": direction,
                "offset": offset,
                "total_count": total,
                "returned_count": transactions.len(),
                "transactions": transactions
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// 按地址查询待打包交易列表（from/to 任一匹配）。
pub(crate) async fn chain_address_pending_txs_handler(
    State(state): State<AppState>,
    Path(address): Path<String>,
    Query(query): Query<ChainAddressTxsQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    if address.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "address 不能为空"
            })),
        );
    }

    let limit = query.limit.unwrap_or(20);
    if limit == 0 || limit > 200 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "limit 必须在 1~200 之间"
            })),
        );
    }
    let direction = query
        .direction
        .as_deref()
        .unwrap_or("all")
        .trim()
        .to_ascii_lowercase();
    let offset = query.offset.unwrap_or(0);
    if direction != "all" && direction != "in" && direction != "out" {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "direction 必须是 all/in/out"
            })),
        );
    }

    match with_chain(&state, |chain| {
        let mut records = Vec::new();
        for tx in chain.pending_transactions.iter().rev() {
            let is_out = tx.from == address;
            let is_in = tx.to == address;
            if !is_out && !is_in {
                continue;
            }

            if direction == "in" && !is_in {
                continue;
            }
            if direction == "out" && !is_out {
                continue;
            }

            let tx_direction = if is_in && is_out {
                "self".to_string()
            } else if is_out {
                "out".to_string()
            } else {
                "in".to_string()
            };

            if direction == "all"
                || direction == tx_direction
                || (tx_direction == "self" && (direction == "in" || direction == "out"))
            {
                records.push(ChainAddressPendingTxRecord {
                    direction: tx_direction,
                    transaction: tx.clone(),
                });
            }
        }
        let total = records.len();
        let returned = records
            .into_iter()
            .skip(offset)
            .take(limit)
            .collect::<Vec<_>>();
        Ok((total, returned))
    }) {
        Ok((total, transactions)) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "address": address,
                "direction": direction,
                "offset": offset,
                "total_count": total,
                "returned_count": transactions.len(),
                "transactions": transactions
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// 地址汇总查询：返回余额与已确认/待打包收支统计。
pub(crate) async fn chain_address_summary_handler(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if address.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "address 不能为空"
            })),
        );
    }

    let balance = match with_state_store(&state, |store| store.get_balance(&address)) {
        Ok(balance) => balance.unwrap_or(0),
        Err((status, body)) => return (status, Json(body)),
    };

    match with_chain(&state, |chain| {
        let mut confirmed_in_count = 0usize;
        let mut confirmed_out_count = 0usize;
        let mut confirmed_in_amount = 0u64;
        let mut confirmed_out_amount = 0u64;
        for block in &chain.chain {
            for tx in &block.transactions {
                if tx.to == address {
                    confirmed_in_count = confirmed_in_count.saturating_add(1);
                    confirmed_in_amount = confirmed_in_amount.saturating_add(tx.amount);
                }
                if tx.from == address {
                    confirmed_out_count = confirmed_out_count.saturating_add(1);
                    confirmed_out_amount = confirmed_out_amount.saturating_add(tx.amount);
                }
            }
        }

        let mut pending_in_count = 0usize;
        let mut pending_out_count = 0usize;
        let mut pending_in_amount = 0u64;
        let mut pending_out_amount = 0u64;
        for tx in &chain.pending_transactions {
            if tx.to == address {
                pending_in_count = pending_in_count.saturating_add(1);
                pending_in_amount = pending_in_amount.saturating_add(tx.amount);
            }
            if tx.from == address {
                pending_out_count = pending_out_count.saturating_add(1);
                pending_out_amount = pending_out_amount.saturating_add(tx.amount);
            }
        }

        Ok((
            confirmed_in_count,
            confirmed_out_count,
            confirmed_in_amount,
            confirmed_out_amount,
            pending_in_count,
            pending_out_count,
            pending_in_amount,
            pending_out_amount,
        ))
    }) {
        Ok((
            confirmed_in_count,
            confirmed_out_count,
            confirmed_in_amount,
            confirmed_out_amount,
            pending_in_count,
            pending_out_count,
            pending_in_amount,
            pending_out_amount,
        )) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "address": address,
                "balance": balance,
                "confirmed_in_count": confirmed_in_count,
                "confirmed_out_count": confirmed_out_count,
                "confirmed_in_amount": confirmed_in_amount,
                "confirmed_out_amount": confirmed_out_amount,
                "pending_in_count": pending_in_count,
                "pending_out_count": pending_out_count,
                "pending_in_amount": pending_in_amount,
                "pending_out_amount": pending_out_amount
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// 统一交易查询接口：优先查询待打包交易，其次查询历史交易。
pub(crate) async fn chain_tx_query_handler(
    State(state): State<AppState>,
    Path(tx_id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if tx_id.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "tx_id 不能为空"
            })),
        );
    }

    match with_chain(&state, |chain| {
        Ok(chain
            .pending_transactions
            .iter()
            .find(|tx| tx.id == tx_id)
            .cloned())
    }) {
        Ok(Some(transaction)) => {
            return (
                StatusCode::OK,
                Json(json!({
                    "ok": true,
                    "source": "pending",
                    "transaction": transaction
                })),
            );
        }
        Ok(None) => {}
        Err((status, body)) => return (status, Json(body)),
    }

    match with_history(&state, |history| history.get_transaction(&tx_id)) {
        Ok(Some(raw)) => match bincode::deserialize::<Transaction>(&raw) {
            Ok(transaction) => (
                StatusCode::OK,
                Json(json!({
                    "ok": true,
                    "source": "history",
                    "transaction": transaction
                })),
            ),
            Err(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "ok": false,
                    "error": format!("历史交易反序列化失败: {error}")
                })),
            ),
        },
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "ok": false,
                "error": format!("交易不存在: {tx_id}")
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// 链上余额查询接口。
pub(crate) async fn chain_balance_handler(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if address.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "address 不能为空"
            })),
        );
    }

    match with_state_store(&state, |store| store.get_balance(&address)) {
        Ok(balance) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "address": address,
                "balance": balance.unwrap_or(0)
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// 合约状态查询接口。
pub(crate) async fn chain_contract_state_handler(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if address.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "address 不能为空"
            })),
        );
    }

    match with_state_store(&state, |store| {
        store.get_contract_state(&address, CONTRACT_SNAPSHOT_FIELD)
    }) {
        Ok(Some(raw)) => match bincode::deserialize::<HashMap<String, i64>>(&raw) {
            Ok(snapshot) => (
                StatusCode::OK,
                Json(json!({
                    "ok": true,
                    "address": address,
                    "state": snapshot
                })),
            ),
            Err(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "ok": false,
                    "error": format!("合约状态反序列化失败: {error}")
                })),
            ),
        },
        Ok(None) => match with_chain(&state, |chain| Ok(chain.contract_state_snapshot(&address))) {
            Ok(snapshot) => (
                StatusCode::OK,
                Json(json!({
                    "ok": true,
                    "address": address,
                    "state": snapshot.unwrap_or_default()
                })),
            ),
            Err((status, body)) => (status, Json(body)),
        },
        Err((status, body)) => (status, Json(body)),
    }
}

/// 合约事件查询接口。
pub(crate) async fn chain_contract_events_handler(
    State(state): State<AppState>,
    Path(address): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if address.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "address 不能为空"
            })),
        );
    }

    match with_state_store(&state, |store| {
        store.get_contract_state(&address, CONTRACT_EVENTS_FIELD)
    }) {
        Ok(Some(raw)) => match bincode::deserialize::<Vec<String>>(&raw) {
            Ok(events) => (
                StatusCode::OK,
                Json(json!({
                    "ok": true,
                    "address": address,
                    "events": events
                })),
            ),
            Err(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "ok": false,
                    "error": format!("合约事件反序列化失败: {error}")
                })),
            ),
        },
        Ok(None) => {
            match with_chain(&state, |chain| Ok(chain.contract_events_snapshot(&address))) {
                Ok(events) => (
                    StatusCode::OK,
                    Json(json!({
                        "ok": true,
                        "address": address,
                        "events": events
                    })),
                ),
                Err((status, body)) => (status, Json(body)),
            }
        }
        Err((status, body)) => (status, Json(body)),
    }
}

/// 合约状态字段查询接口。
pub(crate) async fn chain_contract_field_handler(
    State(state): State<AppState>,
    Path((address, field)): Path<(String, String)>,
) -> (StatusCode, Json<serde_json::Value>) {
    if address.trim().is_empty() || field.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "address 和 field 不能为空"
            })),
        );
    }

    match with_state_store(&state, |store| store.get_contract_state(&address, &field)) {
        Ok(Some(raw)) => {
            let i64_value = decode_i64_from_le_bytes(&raw);
            (
                StatusCode::OK,
                Json(json!({
                    "ok": true,
                    "address": address,
                    "field": field,
                    "value_bytes": raw,
                    "value_i64": i64_value
                })),
            )
        }
        Ok(None) => match with_chain(&state, |chain| Ok(chain.contract_state_snapshot(&address))) {
            Ok(Some(snapshot)) => match snapshot.get(&field) {
                Some(value) => (
                    StatusCode::OK,
                    Json(json!({
                        "ok": true,
                        "address": address,
                        "field": field,
                        "value_bytes": value.to_le_bytes(),
                        "value_i64": value
                    })),
                ),
                None => (
                    StatusCode::NOT_FOUND,
                    Json(json!({
                        "ok": false,
                        "error": format!("合约字段不存在: address={address}, field={field}")
                    })),
                ),
            },
            Ok(None) => (
                StatusCode::NOT_FOUND,
                Json(json!({
                    "ok": false,
                    "error": format!("合约字段不存在: address={address}, field={field}")
                })),
            ),
            Err((status, body)) => (status, Json(body)),
        },
        Err((status, body)) => (status, Json(body)),
    }
}

/// 链交易提交接口。
pub(crate) async fn chain_submit_tx_handler(
    State(state): State<AppState>,
    Json(payload): Json<ChainSubmitTxRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let tx = payload.transaction.clone();
    match with_chain_mut(&state, |chain| {
        chain.add_transaction(tx.clone())?;
        Ok(chain.pending_transactions.len())
    }) {
        Ok(pending_tx_count) => {
            let tx_bytes = match bincode::serialize(&tx) {
                Ok(bytes) => bytes,
                Err(error) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({
                            "ok": false,
                            "error": format!("交易序列化失败: {error}")
                        })),
                    );
                }
            };
            let broadcast = match with_p2p(&state, |engine| {
                Ok(
                    engine.broadcast_to_connected(NetworkMessage::NewTransaction {
                        transaction: tx_bytes,
                    }),
                )
            }) {
                Ok(outbound) => outbound,
                Err((status, body)) => return (status, Json(body)),
            };

            (
                StatusCode::OK,
                Json(json!({
                    "ok": true,
                    "pending_tx_count": pending_tx_count,
                    "p2p_outbound_count": broadcast.len(),
                    "p2p_outbound": broadcast
                })),
            )
        }
        Err((status, body)) => (status, Json(body)),
    }
}

/// 手动挖矿接口。
pub(crate) async fn chain_mine_handler(
    State(state): State<AppState>,
    Json(payload): Json<ChainMineRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if payload.miner_address.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "miner_address 不能为空"
            })),
        );
    }

    let history_store = state.history_store.clone();
    let state_store = state.state_store.clone();
    match with_chain_mut(&state, |chain| {
        let block = chain.mine_pending_transactions(&payload.miner_address)?;
        let chain_status = chain_status_from_blockchain(chain);
        Ok((
            block,
            chain_status,
            chain.balances(),
            chain.contract_states.clone(),
            chain.contract_events.clone(),
        ))
    }) {
        Ok((block, chain_status, balances, contract_states, contract_events)) => {
            if let Err(error) = persist_mined_block(history_store.as_ref(), &block) {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "ok": false,
                        "error": format!("持久化历史数据失败: {error}")
                    })),
                );
            }
            if let Err(error) = persist_runtime_state(
                state_store.as_ref(),
                &balances,
                &contract_states,
                &contract_events,
            ) {
                return (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    Json(json!({
                        "ok": false,
                        "error": format!("持久化状态数据失败: {error}")
                    })),
                );
            }

            let block_bytes = match bincode::serialize(&block) {
                Ok(bytes) => bytes,
                Err(error) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({
                            "ok": false,
                            "error": format!("区块序列化失败: {error}")
                        })),
                    );
                }
            };
            let broadcast = match with_p2p_mut(&state, |engine| {
                engine.update_local_chain_status(chain_status.clone());
                Ok(engine.broadcast_to_connected(NetworkMessage::NewBlock { block: block_bytes }))
            }) {
                Ok(outbound) => outbound,
                Err((status, body)) => return (status, Json(body)),
            };

            (
                StatusCode::OK,
                Json(json!({
                    "ok": true,
                    "block": {
                        "index": block.index,
                        "hash": block.hash,
                        "previous_hash": block.previous_hash,
                        "tx_count": block.transactions.len(),
                        "difficulty": block.difficulty,
                        "nonce": block.nonce
                    },
                    "p2p_outbound_count": broadcast.len(),
                    "p2p_outbound": broadcast
                })),
            )
        }
        Err((status, body)) => (status, Json(body)),
    }
}

/// 历史区块查询接口。
pub(crate) async fn history_block_handler(
    State(state): State<AppState>,
    Path(block_hash): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if block_hash.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "block_hash 不能为空"
            })),
        );
    }

    match with_history(&state, |history| history.get_block(&block_hash)) {
        Ok(Some(raw)) => match bincode::deserialize::<rustchain_core::block::Block>(&raw) {
            Ok(block) => (
                StatusCode::OK,
                Json(json!({
                    "ok": true,
                    "block": block
                })),
            ),
            Err(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "ok": false,
                    "error": format!("历史区块反序列化失败: {error}")
                })),
            ),
        },
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "ok": false,
                "error": format!("区块不存在: {block_hash}")
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// 历史交易查询接口。
pub(crate) async fn history_tx_handler(
    State(state): State<AppState>,
    Path(tx_id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    if tx_id.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "tx_id 不能为空"
            })),
        );
    }

    match with_history(&state, |history| history.get_transaction(&tx_id)) {
        Ok(Some(raw)) => match bincode::deserialize::<Transaction>(&raw) {
            Ok(transaction) => (
                StatusCode::OK,
                Json(json!({
                    "ok": true,
                    "transaction": transaction
                })),
            ),
            Err(error) => (
                StatusCode::INTERNAL_SERVER_ERROR,
                Json(json!({
                    "ok": false,
                    "error": format!("历史交易反序列化失败: {error}")
                })),
            ),
        },
        Ok(None) => (
            StatusCode::NOT_FOUND,
            Json(json!({
                "ok": false,
                "error": format!("交易不存在: {tx_id}")
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}
