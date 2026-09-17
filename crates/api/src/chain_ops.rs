//! 链侧共享操作：持久化、编码与同步区块接收。
//!
//! 这些函数被多个 handler 复用（例如 P2P 入站消息与链接口都要写历史、改状态），
//! 集中在此以保证"链变更 → 持久化 → 更新 P2P 摘要"的顺序只有一种实现。

use crate::error::{map_storage_error, ApiError};
use crate::state::{chain_status_from_blockchain, with_chain_mut, with_p2p_mut, AppState};
use rustchain_core::block::Block;
use rustchain_core::transaction::Transaction;
use rustchain_p2p::message::NetworkMessage;
use rustchain_storage::error::StorageError;
use rustchain_storage::history::HistoryStore;
use rustchain_storage::state::StateStore;
use std::collections::HashMap;

/// 合约状态快照字段名。
pub(crate) const CONTRACT_SNAPSHOT_FIELD: &str = "__snapshot__";
/// 合约事件快照字段名。
pub(crate) const CONTRACT_EVENTS_FIELD: &str = "__events__";

/// 持久化新挖出的区块及其交易历史。
pub(crate) fn persist_mined_block(
    history: &dyn HistoryStore,
    block: &Block,
) -> Result<(), StorageError> {
    let block_bytes =
        bincode::serialize(block).map_err(|error| StorageError::Codec(error.to_string()))?;
    history.put_block(&block.hash, &block_bytes)?;

    for tx in &block.transactions {
        let tx_bytes =
            bincode::serialize(tx).map_err(|error| StorageError::Codec(error.to_string()))?;
        history.put_transaction(&tx.id, &tx_bytes)?;
    }

    Ok(())
}

/// 持久化当前链状态（余额与合约状态/事件）。
pub(crate) fn persist_runtime_state(
    state_store: &dyn StateStore,
    balances: &HashMap<String, u64>,
    contract_states: &HashMap<String, HashMap<String, i64>>,
    contract_events: &HashMap<String, Vec<String>>,
) -> Result<(), StorageError> {
    for (address, balance) in balances {
        state_store.set_balance(address, *balance)?;
    }

    for (contract, fields) in contract_states {
        for (field, value) in fields {
            state_store.set_contract_state(contract, field, &value.to_le_bytes())?;
        }

        let snapshot =
            bincode::serialize(fields).map_err(|error| StorageError::Codec(error.to_string()))?;
        state_store.set_contract_state(contract, CONTRACT_SNAPSHOT_FIELD, &snapshot)?;
    }

    for (contract, events) in contract_events {
        let raw_events =
            bincode::serialize(events).map_err(|error| StorageError::Codec(error.to_string()))?;
        state_store.set_contract_state(contract, CONTRACT_EVENTS_FIELD, &raw_events)?;
    }

    Ok(())
}

/// 编码区块列表为网络传输字节。
pub(crate) fn encode_blocks(blocks: Vec<Block>) -> Result<Vec<Vec<u8>>, bincode::Error> {
    blocks
        .into_iter()
        .map(|block| bincode::serialize(&block))
        .collect()
}

/// 编码交易列表为网络传输字节。
pub(crate) fn encode_transactions(
    transactions: Vec<Transaction>,
) -> Result<Vec<Vec<u8>>, bincode::Error> {
    transactions
        .into_iter()
        .map(|tx| bincode::serialize(&tx))
        .collect()
}

/// 接收同步区块并完成链、历史、状态和 P2P 摘要更新。
///
/// 顺序不可调换：先追加到内存链（含全部校验），再落盘，最后刷新 P2P 链摘要，
/// 保证任一步失败时不会对外广播一个未持久化的高度。
pub(crate) fn accept_synced_block(state: &AppState, block: Block) -> Result<(), ApiError> {
    let history_store = state.history_store.clone();
    let state_store = state.state_store.clone();
    let (chain_status, balances, contract_states, contract_events) =
        with_chain_mut(state, |chain| {
            chain.append_external_block(block.clone())?;
            Ok((
                chain_status_from_blockchain(chain),
                chain.balances(),
                chain.contract_states.clone(),
                chain.contract_events.clone(),
            ))
        })?;

    persist_mined_block(history_store.as_ref(), &block).map_err(map_storage_error)?;
    persist_runtime_state(
        state_store.as_ref(),
        &balances,
        &contract_states,
        &contract_events,
    )
    .map_err(map_storage_error)?;
    with_p2p_mut(state, |engine| {
        engine.update_local_chain_status(chain_status);
        Ok(())
    })?;

    Ok(())
}

/// 根据本地/远端高度判断是否需要继续拉取下一批区块。
pub(crate) fn build_next_get_blocks_request(
    state: &AppState,
    peer_id: &str,
) -> Result<Option<NetworkMessage>, ApiError> {
    let local_height = crate::state::with_chain(state, |chain| Ok(chain.latest_block()?.index))?;
    let peer_best_height = crate::state::with_p2p(state, |engine| {
        Ok(engine.peers().get(peer_id).map(|p| p.best_height))
    })?;

    let Some(peer_best_height) = peer_best_height else {
        return Ok(None);
    };
    if peer_best_height <= local_height {
        return Ok(None);
    }

    // 单批最多 128，和同步引擎默认请求窗口保持一致，便于分批追平。
    let remaining = peer_best_height.saturating_sub(local_height);
    let limit = remaining.min(128) as u32;
    Ok(Some(NetworkMessage::GetBlocks {
        from_height: local_height.saturating_add(1),
        limit,
    }))
}

/// 将 8 字节小端编码解码为 i64，长度不合法时返回 None。
pub(crate) fn decode_i64_from_le_bytes(raw: &[u8]) -> Option<i64> {
    if raw.len() != 8 {
        return None;
    }

    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(raw);
    Some(i64::from_le_bytes(bytes))
}
