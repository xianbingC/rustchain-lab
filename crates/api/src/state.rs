//! API 进程内共享状态与访问辅助函数。
//!
//! 原型阶段所有可变状态都放在 `Arc<Mutex<...>>` 中，这里集中提供
//! `with_*` 访问器，把"取锁 + 错误映射"的重复逻辑收敛到一处。

use crate::error::{
    lock_error, map_core_error, map_nft_error, map_p2p_error, map_storage_error, ApiError,
};
use rustchain_apps::nft::{NftError, NftMarketplace};
use rustchain_common::{AppConfig, AppResult};
use rustchain_core::block::Block;
use rustchain_core::blockchain::Blockchain;
use rustchain_p2p::engine::SyncEngine;
use rustchain_p2p::message::ChainStatus;
use rustchain_p2p::transport::TransportSessionPool;
#[cfg(any(test, not(feature = "rocksdb-backend")))]
use rustchain_storage::state::InMemoryStateStore;
use rustchain_storage::{
    history::{HistoryStore, LevelDbHistoryStore},
    state::StateStore,
};
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
};

/// P2P 协议版本号（原型阶段固定）。
pub(crate) const P2P_PROTOCOL_VERSION: &str = "1.0.0";

/// API 进程内共享状态。
#[derive(Clone)]
pub(crate) struct AppState {
    /// 区块链状态（原型阶段使用内存存储）。
    pub(crate) blockchain: Arc<Mutex<Blockchain>>,
    /// P2P 同步引擎。
    pub(crate) p2p_engine: Arc<Mutex<SyncEngine>>,
    /// P2P 传输会话池，用于缓存半包和维护连接内序号。
    pub(crate) p2p_transport_sessions: Arc<Mutex<TransportSessionPool>>,
    /// 本地 P2P 监听地址，用于构造握手消息。
    pub(crate) p2p_listen_addr: String,
    /// 本地 P2P 协议版本，用于构造握手消息。
    pub(crate) p2p_protocol_version: String,
    /// 状态存储（余额与合约状态）。
    pub(crate) state_store: Arc<dyn StateStore + Send + Sync>,
    /// 历史数据存储。
    pub(crate) history_store: Arc<dyn HistoryStore + Send + Sync>,
    /// NFT 市场（原型阶段使用内存存储）。
    pub(crate) nft_marketplace: Arc<Mutex<NftMarketplace>>,
}

/// 构造默认应用状态（仅供测试使用）。
#[cfg(test)]
pub(crate) fn default_app_state() -> AppState {
    let blockchain = Blockchain::new(2, 50);
    let chain_status = chain_status_from_blockchain(&blockchain);
    AppState {
        blockchain: Arc::new(Mutex::new(blockchain)),
        p2p_engine: Arc::new(Mutex::new(SyncEngine::new("api-test-node", chain_status))),
        p2p_transport_sessions: Arc::new(Mutex::new(TransportSessionPool::new())),
        p2p_listen_addr: "0.0.0.0:7000".to_string(),
        p2p_protocol_version: P2P_PROTOCOL_VERSION.to_string(),
        state_store: Arc::new(InMemoryStateStore::new()),
        history_store: Arc::new(rustchain_storage::history::InMemoryHistoryStore::new()),
        nft_marketplace: Arc::new(Mutex::new(NftMarketplace::new())),
    }
}

/// 根据配置构造应用状态。
pub(crate) fn default_app_state_with_config(config: &AppConfig) -> AppResult<AppState> {
    let mut blockchain = Blockchain::new(config.mining_difficulty, config.mining_reward);
    blockchain.target_block_time_secs = config.target_block_time_secs;
    blockchain.difficulty_adjustment_interval = config.difficulty_adjustment_interval;
    let chain_status = chain_status_from_blockchain(&blockchain);
    let local_peer_id = format!("{}-node", config.app_name);
    let mut p2p_engine = SyncEngine::new(local_peer_id, chain_status);
    let seeded_peer_count =
        bootstrap_seed_nodes(&mut p2p_engine, &mut blockchain, &config.seed_nodes);
    if seeded_peer_count > 0 {
        tracing::info!(seeded_peer_count, "已加载种子节点到 P2P 引擎");
    }
    let state_store = open_state_store(config)?;
    let history_store = open_history_store(config)?;
    Ok(AppState {
        blockchain: Arc::new(Mutex::new(blockchain)),
        p2p_engine: Arc::new(Mutex::new(p2p_engine)),
        p2p_transport_sessions: Arc::new(Mutex::new(TransportSessionPool::new())),
        p2p_listen_addr: config.p2p_bind_addr.clone(),
        p2p_protocol_version: P2P_PROTOCOL_VERSION.to_string(),
        state_store,
        history_store,
        nft_marketplace: Arc::new(Mutex::new(NftMarketplace::new())),
    })
}

/// 打开状态存储（RocksDB 或内存实现）。
#[cfg(feature = "rocksdb-backend")]
fn open_state_store(config: &AppConfig) -> AppResult<Arc<dyn StateStore + Send + Sync>> {
    let path = PathBuf::from(&config.data_dir).join("state-rocksdb");
    let store = rustchain_storage::state::RocksDbStateStore::open(&path).map_err(|error| {
        rustchain_common::AppError::Command(format!(
            "打开状态存储失败: path={}, error={error}",
            path.display()
        ))
    })?;
    Ok(Arc::new(store))
}

/// 打开状态存储（未启用 RocksDB 时回退内存实现）。
#[cfg(not(feature = "rocksdb-backend"))]
fn open_state_store(_config: &AppConfig) -> AppResult<Arc<dyn StateStore + Send + Sync>> {
    tracing::warn!("未启用 rocksdb-backend，状态存储使用内存实现");
    Ok(Arc::new(InMemoryStateStore::new()))
}

/// 打开历史存储（LevelDB）。
fn open_history_store(config: &AppConfig) -> AppResult<Arc<dyn HistoryStore + Send + Sync>> {
    let path = PathBuf::from(&config.data_dir).join("history-leveldb");
    let store = LevelDbHistoryStore::open(&path).map_err(|error| {
        rustchain_common::AppError::Command(format!(
            "打开历史存储失败: path={}, error={error}",
            path.display()
        ))
    })?;
    Ok(Arc::new(store))
}

/// 根据当前链状态生成 P2P 链摘要。
pub(crate) fn chain_status_from_blockchain(chain: &Blockchain) -> ChainStatus {
    let latest = chain
        .latest_block()
        .cloned()
        .unwrap_or_else(|_| Block::genesis());
    let next_difficulty = chain
        .next_block_expected_difficulty()
        .unwrap_or(chain.difficulty);
    ChainStatus {
        chain_id: chain.chain_id.clone(),
        best_height: latest.index,
        best_hash: latest.hash,
        difficulty: next_difficulty,
        genesis_hash: Block::genesis().hash,
    }
}

/// 将配置中的种子节点导入同步引擎和链节点列表。
pub(crate) fn bootstrap_seed_nodes(
    engine: &mut SyncEngine,
    chain: &mut Blockchain,
    seed_nodes: &[String],
) -> usize {
    let mut imported = 0usize;
    for (index, raw) in seed_nodes.iter().enumerate() {
        let Some((peer_id, address)) = parse_seed_node_entry(raw, index) else {
            continue;
        };

        let before_count = engine.peer_count();
        engine.register_peer(peer_id, address.clone());
        chain.add_peer(address);
        if engine.peer_count() > before_count {
            imported = imported.saturating_add(1);
        }
    }

    imported
}

/// 解析种子节点配置项，支持 `peer_id@address` 与纯地址两种格式。
pub(crate) fn parse_seed_node_entry(raw: &str, index: usize) -> Option<(String, String)> {
    let value = raw.trim();
    if value.is_empty() {
        return None;
    }

    if let Some((peer_id, address)) = value.split_once('@') {
        let peer_id = peer_id.trim();
        let address = address.trim();
        if peer_id.is_empty() || address.is_empty() {
            return None;
        }
        return Some((peer_id.to_string(), address.to_string()));
    }

    // 仅提供地址时自动生成稳定的种子节点 ID，便于后续查询和调试。
    Some((format!("seed-{}", index + 1), value.to_string()))
}

/// 只读访问区块链状态。
pub(crate) fn with_chain<T>(
    state: &AppState,
    f: impl FnOnce(&Blockchain) -> Result<T, rustchain_core::error::CoreError>,
) -> Result<T, ApiError> {
    let guard = state
        .blockchain
        .lock()
        .map_err(|_| lock_error("blockchain"))?;

    f(&guard).map_err(map_core_error)
}

/// 可变访问区块链状态。
pub(crate) fn with_chain_mut<T>(
    state: &AppState,
    f: impl FnOnce(&mut Blockchain) -> Result<T, rustchain_core::error::CoreError>,
) -> Result<T, ApiError> {
    let mut guard = state
        .blockchain
        .lock()
        .map_err(|_| lock_error("blockchain"))?;

    f(&mut guard).map_err(map_core_error)
}

/// 只读访问 P2P 引擎。
pub(crate) fn with_p2p<T>(
    state: &AppState,
    f: impl FnOnce(&SyncEngine) -> Result<T, rustchain_p2p::P2pError>,
) -> Result<T, ApiError> {
    let guard = state
        .p2p_engine
        .lock()
        .map_err(|_| lock_error("p2p_engine"))?;

    f(&guard).map_err(map_p2p_error)
}

/// 可变访问 P2P 引擎。
pub(crate) fn with_p2p_mut<T>(
    state: &AppState,
    f: impl FnOnce(&mut SyncEngine) -> Result<T, rustchain_p2p::P2pError>,
) -> Result<T, ApiError> {
    let mut guard = state
        .p2p_engine
        .lock()
        .map_err(|_| lock_error("p2p_engine"))?;

    f(&mut guard).map_err(map_p2p_error)
}

/// 只读访问 P2P 传输会话池。
pub(crate) fn with_transport_sessions<T>(
    state: &AppState,
    f: impl FnOnce(&TransportSessionPool) -> Result<T, rustchain_p2p::P2pError>,
) -> Result<T, ApiError> {
    let guard = state
        .p2p_transport_sessions
        .lock()
        .map_err(|_| lock_error("p2p_transport_sessions"))?;

    f(&guard).map_err(map_p2p_error)
}

/// 只读访问 NFT 市场。
pub(crate) fn with_market<T>(
    state: &AppState,
    f: impl FnOnce(&NftMarketplace) -> Result<T, NftError>,
) -> Result<T, ApiError> {
    let guard = state
        .nft_marketplace
        .lock()
        .map_err(|_| lock_error("nft_marketplace"))?;

    f(&guard).map_err(map_nft_error)
}

/// 可变访问 NFT 市场。
pub(crate) fn with_market_mut<T>(
    state: &AppState,
    f: impl FnOnce(&mut NftMarketplace) -> Result<T, NftError>,
) -> Result<T, ApiError> {
    let mut guard = state
        .nft_marketplace
        .lock()
        .map_err(|_| lock_error("nft_marketplace"))?;

    f(&mut guard).map_err(map_nft_error)
}

/// 访问历史存储。
pub(crate) fn with_history<T>(
    state: &AppState,
    f: impl FnOnce(&dyn HistoryStore) -> Result<T, rustchain_storage::error::StorageError>,
) -> Result<T, ApiError> {
    f(state.history_store.as_ref()).map_err(map_storage_error)
}

/// 访问状态存储。
pub(crate) fn with_state_store<T>(
    state: &AppState,
    f: impl FnOnce(&dyn StateStore) -> Result<T, rustchain_storage::error::StorageError>,
) -> Result<T, ApiError> {
    f(state.state_store.as_ref()).map_err(map_storage_error)
}
