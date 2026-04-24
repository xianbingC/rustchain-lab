use axum::{
    extract::{Path, Query, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use rustchain_apps::defi::{DefiError, LendingConfig, LendingPool};
use rustchain_apps::nft::{NftError, NftMarketplace};
use rustchain_common::{logging::init_logging, AppConfig, AppResult};
use rustchain_core::block::Block;
use rustchain_core::blockchain::Blockchain;
use rustchain_core::transaction::Transaction;
use rustchain_crypto::wallet::create_wallet;
use rustchain_p2p::{
    engine::{OutboundEnvelope, SyncEngine},
    message::{ChainStatus, NetworkMessage},
};
use rustchain_storage::{
    error::StorageError,
    history::{HistoryStore, LevelDbHistoryStore},
    state::{InMemoryStateStore, StateStore},
};
use rustchain_vm::{
    compiler::{compile, CompileError},
    runtime::{Runtime, VmError},
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    collections::HashMap,
    path::PathBuf,
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::net::TcpListener;

/// 合约状态快照字段名。
const CONTRACT_SNAPSHOT_FIELD: &str = "__snapshot__";
/// 合约事件快照字段名。
const CONTRACT_EVENTS_FIELD: &str = "__events__";

/// API 程序入口。
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("API 启动失败: {error}");
        std::process::exit(1);
    }
}

/// 执行 API 初始化和监听流程。
async fn run() -> AppResult<()> {
    let config = AppConfig::from_env("rustchain-api")?;
    init_logging(&config)?;

    let app = build_app(default_app_state_with_config(&config)?);
    let listen_addr = config.api_listen_addr();
    let listener = TcpListener::bind(&listen_addr).await?;

    tracing::info!(
        app = %config.app_name,
        listen_addr = %listen_addr,
        p2p_bind_addr = %config.p2p_bind_addr,
        "API 服务启动成功"
    );

    axum::serve(listener, app)
        .await
        .map_err(|error| rustchain_common::AppError::Io(std::io::Error::other(error)))?;

    Ok(())
}

/// 健康检查接口。
async fn health_handler() -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "service": "rustchain-api"
    }))
}

/// 创建钱包请求。
#[derive(Debug, Deserialize)]
struct CreateWalletRequest {
    /// 钱包密码。
    password: String,
}

/// 交易验签请求。
#[derive(Debug, Deserialize)]
struct VerifyTxRequest {
    /// 待校验交易。
    transaction: Transaction,
}

/// 提交链交易请求。
#[derive(Debug, Deserialize)]
struct ChainSubmitTxRequest {
    /// 待提交交易。
    transaction: Transaction,
}

/// 挖矿请求。
#[derive(Debug, Deserialize)]
struct ChainMineRequest {
    /// 矿工地址。
    miner_address: String,
}

/// P2P 节点注册请求。
#[derive(Debug, Deserialize)]
struct P2pRegisterPeerRequest {
    /// 节点 ID。
    peer_id: String,
    /// 节点地址。
    address: String,
}

/// P2P 入站消息请求。
#[derive(Debug, Deserialize)]
struct P2pIncomingMessageRequest {
    /// 来源节点 ID。
    peer_id: String,
    /// 来源节点地址。
    address: String,
    /// 连接内递增序号。
    sequence: u64,
    /// 网络消息内容。
    message: NetworkMessage,
}

/// VM 编译请求。
#[derive(Debug, Deserialize)]
struct VmCompileRequest {
    /// 合约源码文本。
    source: String,
}

/// VM 执行请求。
#[derive(Debug, Deserialize)]
struct VmExecuteRequest {
    /// 合约源码文本。
    source: String,
    /// 可选步数上限。
    max_steps: Option<usize>,
}

/// 链信息响应。
#[derive(Debug, Serialize)]
struct ChainInfoResponse {
    /// 链标识。
    chain_id: String,
    /// 当前链高度。
    height: u64,
    /// 最新区块哈希。
    latest_hash: String,
    /// 当前难度。
    difficulty: u32,
    /// 交易池数量。
    pending_tx_count: usize,
    /// 已连接节点数量。
    peer_count: usize,
}

/// 交易池查询参数。
#[derive(Debug, Default, Deserialize)]
struct ChainMempoolQuery {
    /// 可选返回条数上限（>0）。
    limit: Option<usize>,
    /// 可选地址过滤（匹配 from 或 to）。
    address: Option<String>,
}

/// 区块列表查询参数。
#[derive(Debug, Default, Deserialize)]
struct ChainBlocksQuery {
    /// 起始高度（包含），默认 0。
    from_height: Option<u64>,
    /// 可选返回条数上限（1~200），默认 20。
    limit: Option<usize>,
}

/// 地址交易查询参数。
#[derive(Debug, Default, Deserialize)]
struct ChainAddressTxsQuery {
    /// 可选返回条数上限（1~200），默认 20。
    limit: Option<usize>,
    /// 方向过滤：all/in/out，默认 all。
    direction: Option<String>,
}

/// 地址交易查询项。
#[derive(Debug, Clone, Serialize)]
struct ChainAddressTxRecord {
    /// 所在区块高度。
    block_index: u64,
    /// 所在区块哈希。
    block_hash: String,
    /// 相对查询地址的方向（in/out/self）。
    direction: String,
    /// 交易详情。
    transaction: Transaction,
}

/// 地址待打包交易查询项。
#[derive(Debug, Clone, Serialize)]
struct ChainAddressPendingTxRecord {
    /// 相对查询地址的方向（in/out/self）。
    direction: String,
    /// 交易详情。
    transaction: Transaction,
}

/// 创建钱包接口（原型阶段同时返回密钥对用于学习调试）。
async fn wallet_create_handler(
    Json(payload): Json<CreateWalletRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if payload.password.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "password 不能为空"
            })),
        );
    }

    match create_wallet(&payload.password) {
        Ok((wallet, key_pair)) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "wallet": {
                    "address": wallet.address,
                    "public_key": wallet.public_key,
                    "encrypted_private_key": wallet.encrypted_private_key,
                    "kdf_salt": wallet.kdf_salt,
                    "private_key_checksum": wallet.private_key_checksum
                },
                "key_pair": {
                    "address": key_pair.address,
                    "public_key": key_pair.public_key,
                    "private_key": key_pair.private_key
                }
            })),
        ),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": error.to_string()
            })),
        ),
    }
}

/// API 进程内共享状态。
#[derive(Clone)]
struct AppState {
    /// 区块链状态（原型阶段使用内存存储）。
    blockchain: Arc<Mutex<Blockchain>>,
    /// P2P 同步引擎。
    p2p_engine: Arc<Mutex<SyncEngine>>,
    /// 状态存储（余额与合约状态）。
    state_store: Arc<dyn StateStore + Send + Sync>,
    /// 历史数据存储。
    history_store: Arc<dyn HistoryStore + Send + Sync>,
    /// DeFi 借贷池（原型阶段使用内存存储）。
    lending_pool: Arc<Mutex<LendingPool>>,
    /// NFT 市场（原型阶段使用内存存储）。
    nft_marketplace: Arc<Mutex<NftMarketplace>>,
}

/// 构造默认应用状态。
#[cfg(test)]
fn default_app_state() -> AppState {
    let blockchain = Blockchain::new(2, 50);
    let chain_status = chain_status_from_blockchain(&blockchain);
    AppState {
        blockchain: Arc::new(Mutex::new(blockchain)),
        p2p_engine: Arc::new(Mutex::new(SyncEngine::new("api-test-node", chain_status))),
        state_store: Arc::new(InMemoryStateStore::new()),
        history_store: Arc::new(rustchain_storage::history::InMemoryHistoryStore::new()),
        lending_pool: Arc::new(Mutex::new(LendingPool::new(
            LendingConfig::default(),
            now_unix_ts(),
        ))),
        nft_marketplace: Arc::new(Mutex::new(NftMarketplace::new())),
    }
}

/// 根据配置构造应用状态。
fn default_app_state_with_config(config: &AppConfig) -> AppResult<AppState> {
    let blockchain = Blockchain::new(config.mining_difficulty, config.mining_reward);
    let chain_status = chain_status_from_blockchain(&blockchain);
    let local_peer_id = format!("{}-node", config.app_name);
    let state_store = open_state_store(config)?;
    let history_store = open_history_store(config)?;
    Ok(AppState {
        blockchain: Arc::new(Mutex::new(blockchain)),
        p2p_engine: Arc::new(Mutex::new(SyncEngine::new(local_peer_id, chain_status))),
        state_store,
        history_store,
        lending_pool: Arc::new(Mutex::new(LendingPool::new(
            LendingConfig::default(),
            now_unix_ts(),
        ))),
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
fn chain_status_from_blockchain(chain: &Blockchain) -> ChainStatus {
    let latest = chain
        .latest_block()
        .cloned()
        .unwrap_or_else(|_| Block::genesis());
    ChainStatus {
        chain_id: chain.chain_id.clone(),
        best_height: latest.index,
        best_hash: latest.hash,
        difficulty: chain.difficulty,
        genesis_hash: Block::genesis().hash,
    }
}

/// 构造 API 路由。
fn build_app(shared_state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/wallet/create", post(wallet_create_handler))
        .route("/tx/verify", post(tx_verify_handler))
        .route("/p2p/status", get(p2p_status_handler))
        .route("/p2p/peers", get(p2p_peers_handler))
        .route("/p2p/peer/register", post(p2p_register_peer_handler))
        .route("/p2p/message", post(p2p_message_handler))
        .route("/chain/info", get(chain_info_handler))
        .route("/chain/validate", get(chain_validate_handler))
        .route("/chain/block/latest", get(chain_latest_block_handler))
        .route("/chain/blocks", get(chain_blocks_handler))
        .route("/chain/block/:height", get(chain_block_by_height_handler))
        .route(
            "/chain/address/:address/txs",
            get(chain_address_txs_handler),
        )
        .route(
            "/chain/address/:address/summary",
            get(chain_address_summary_handler),
        )
        .route(
            "/chain/address/:address/pending-txs",
            get(chain_address_pending_txs_handler),
        )
        .route("/chain/pending-tx/:tx_id", get(chain_pending_tx_handler))
        .route("/chain/tx/:tx_id", get(chain_tx_query_handler))
        .route("/chain/mempool", get(chain_mempool_handler))
        .route("/chain/balance/:address", get(chain_balance_handler))
        .route(
            "/chain/contract/:address/state",
            get(chain_contract_state_handler),
        )
        .route(
            "/chain/contract/:address/events",
            get(chain_contract_events_handler),
        )
        .route(
            "/chain/contract/:address/field/:field",
            get(chain_contract_field_handler),
        )
        .route("/chain/tx", post(chain_submit_tx_handler))
        .route("/chain/mine", post(chain_mine_handler))
        .route("/history/block/:block_hash", get(history_block_handler))
        .route("/history/tx/:tx_id", get(history_tx_handler))
        .route("/vm/compile", post(vm_compile_handler))
        .route("/vm/execute", post(vm_execute_handler))
        .route("/defi/deposit", post(defi_deposit_handler))
        .route("/defi/borrow", post(defi_borrow_handler))
        .route("/defi/repay", post(defi_repay_handler))
        .route("/defi/withdraw", post(defi_withdraw_handler))
        .route("/defi/liquidate", post(defi_liquidate_handler))
        .route("/defi/position/:owner", get(defi_position_handler))
        .route("/defi/stats", get(defi_stats_handler))
        .route("/nft/mint", post(nft_mint_handler))
        .route("/nft/list", post(nft_list_handler))
        .route("/nft/cancel", post(nft_cancel_handler))
        .route("/nft/buy", post(nft_buy_handler))
        .route("/nft/token/:token_id", get(nft_token_handler))
        .route("/nft/listing/:listing_id", get(nft_listing_handler))
        .route("/nft/listings/active", get(nft_active_listings_handler))
        .route("/nft/owner/:owner/tokens", get(nft_owner_tokens_handler))
        .with_state(shared_state)
}

/// 交易验签接口：检查交易结构、地址公钥匹配和签名有效性。
async fn tx_verify_handler(
    Json(payload): Json<VerifyTxRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    match payload.transaction.validate_for_chain() {
        Ok(_) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "valid": true
            })),
        ),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "valid": false,
                "error": error.to_string()
            })),
        ),
    }
}

/// P2P 状态查询接口。
async fn p2p_status_handler(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
    match with_p2p(&state, |engine| {
        Ok(json!({
            "ok": true,
            "local_peer_id": engine.local_peer_id(),
            "local_chain_status": engine.local_chain_status(),
            "peer_count": engine.peer_count()
        }))
    }) {
        Ok(body) => (StatusCode::OK, Json(body)),
        Err((status, body)) => (status, Json(body)),
    }
}

/// P2P 节点列表查询接口。
async fn p2p_peers_handler(State(state): State<AppState>) -> (StatusCode, Json<serde_json::Value>) {
    match with_p2p(&state, |engine| {
        Ok(json!({
            "ok": true,
            "peers": engine.peer_snapshot()
        }))
    }) {
        Ok(body) => (StatusCode::OK, Json(body)),
        Err((status, body)) => (status, Json(body)),
    }
}

/// P2P 节点注册接口。
async fn p2p_register_peer_handler(
    State(state): State<AppState>,
    Json(payload): Json<P2pRegisterPeerRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if payload.peer_id.trim().is_empty() || payload.address.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "peer_id 和 address 不能为空"
            })),
        );
    }

    let peer_address = payload.address.trim().to_string();
    let peer_id = payload.peer_id.trim().to_string();
    match with_p2p_mut(&state, |engine| {
        engine.register_peer(peer_id.clone(), peer_address.clone());
        Ok(engine.peer_count())
    }) {
        Ok(peer_count) => {
            if let Err((status, body)) = with_chain_mut(&state, |chain| {
                chain.add_peer(peer_address.clone());
                Ok(())
            }) {
                return (status, Json(body));
            }
            (
                StatusCode::OK,
                Json(json!({
                    "ok": true,
                    "peer_count": peer_count
                })),
            )
        }
        Err((status, body)) => (status, Json(body)),
    }
}

/// P2P 入站消息模拟接口。
async fn p2p_message_handler(
    State(state): State<AppState>,
    Json(payload): Json<P2pIncomingMessageRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if payload.peer_id.trim().is_empty() || payload.address.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "peer_id 和 address 不能为空"
            })),
        );
    }

    let peer_id = payload.peer_id.trim().to_string();
    let peer_address = payload.address.trim().to_string();
    let incoming_message = payload.message.clone();
    let mut report = match with_p2p_mut(&state, |engine| {
        engine.on_incoming_message(
            peer_id.clone(),
            peer_address,
            payload.sequence,
            incoming_message.clone(),
        )
    }) {
        Ok(report) => report,
        Err((status, body)) => return (status, Json(body)),
    };

    match incoming_message {
        NetworkMessage::NewTransaction { transaction } => {
            let tx = match bincode::deserialize::<Transaction>(&transaction) {
                Ok(tx) => tx,
                Err(error) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "ok": false,
                            "error": format!("反序列化交易失败: {error}")
                        })),
                    );
                }
            };
            if let Err((status, body)) = with_chain_mut(&state, |chain| {
                chain.add_transaction(tx)?;
                Ok(())
            }) {
                return (status, Json(body));
            }
        }
        NetworkMessage::NewBlock { block } => {
            let block = match bincode::deserialize::<Block>(&block) {
                Ok(block) => block,
                Err(error) => {
                    return (
                        StatusCode::BAD_REQUEST,
                        Json(json!({
                            "ok": false,
                            "error": format!("反序列化区块失败: {error}")
                        })),
                    );
                }
            };
            if let Err((status, body)) = accept_synced_block(&state, block) {
                return (status, Json(body));
            }
        }
        NetworkMessage::Blocks { blocks } => {
            let received_count = blocks.len();
            for raw in blocks {
                let block = match bincode::deserialize::<Block>(&raw) {
                    Ok(block) => block,
                    Err(error) => {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({
                                "ok": false,
                                "error": format!("反序列化区块失败: {error}")
                            })),
                        );
                    }
                };
                if let Err((status, body)) = accept_synced_block(&state, block) {
                    return (status, Json(body));
                }
            }

            // 仅在本次确实收到区块时尝试续拉，避免对空响应形成无意义循环请求。
            if received_count > 0 {
                let next_request = match build_next_get_blocks_request(&state, &peer_id) {
                    Ok(request) => request,
                    Err((status, body)) => return (status, Json(body)),
                };
                if let Some(message) = next_request {
                    report.outbound.push(OutboundEnvelope {
                        target_peer_id: peer_id.clone(),
                        message,
                    });
                }
            }
        }
        NetworkMessage::Mempool { transactions } => {
            for raw in transactions {
                let tx = match bincode::deserialize::<Transaction>(&raw) {
                    Ok(tx) => tx,
                    Err(error) => {
                        return (
                            StatusCode::BAD_REQUEST,
                            Json(json!({
                                "ok": false,
                                "error": format!("反序列化交易失败: {error}")
                            })),
                        );
                    }
                };
                if let Err((status, body)) = with_chain_mut(&state, |chain| {
                    chain.add_transaction(tx)?;
                    Ok(())
                }) {
                    return (status, Json(body));
                }
            }
        }
        NetworkMessage::GetMempool => {
            let transactions =
                match with_chain(&state, |chain| Ok(chain.pending_transactions.clone())) {
                    Ok(transactions) => transactions,
                    Err((status, body)) => return (status, Json(body)),
                };
            let encoded_transactions = match encode_transactions(transactions) {
                Ok(encoded) => encoded,
                Err(error) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({
                            "ok": false,
                            "error": format!("交易编码失败: {error}")
                        })),
                    );
                }
            };
            let mut replaced = false;
            for envelope in &mut report.outbound {
                if envelope.target_peer_id == peer_id
                    && matches!(envelope.message, NetworkMessage::Mempool { .. })
                {
                    envelope.message = NetworkMessage::Mempool {
                        transactions: encoded_transactions.clone(),
                    };
                    replaced = true;
                    break;
                }
            }
            if !replaced {
                report.outbound.push(OutboundEnvelope {
                    target_peer_id: peer_id.clone(),
                    message: NetworkMessage::Mempool {
                        transactions: encoded_transactions,
                    },
                });
            }
        }
        NetworkMessage::GetBlocks { from_height, limit } => {
            let blocks = match with_chain(&state, |chain| {
                Ok(chain
                    .chain
                    .iter()
                    .filter(|block| block.index >= from_height)
                    .take(limit as usize)
                    .cloned()
                    .collect::<Vec<_>>())
            }) {
                Ok(blocks) => blocks,
                Err((status, body)) => return (status, Json(body)),
            };
            let encoded_blocks = match encode_blocks(blocks) {
                Ok(encoded) => encoded,
                Err(error) => {
                    return (
                        StatusCode::INTERNAL_SERVER_ERROR,
                        Json(json!({
                            "ok": false,
                            "error": format!("区块编码失败: {error}")
                        })),
                    );
                }
            };
            let mut replaced = false;
            for envelope in &mut report.outbound {
                if envelope.target_peer_id == peer_id
                    && matches!(envelope.message, NetworkMessage::Blocks { .. })
                {
                    envelope.message = NetworkMessage::Blocks {
                        blocks: encoded_blocks.clone(),
                    };
                    replaced = true;
                    break;
                }
            }
            if !replaced {
                report.outbound.push(OutboundEnvelope {
                    target_peer_id: peer_id,
                    message: NetworkMessage::Blocks {
                        blocks: encoded_blocks,
                    },
                });
            }
        }
        _ => {}
    }

    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "processed": report.processed,
            "outbound_count": report.outbound.len(),
            "outbound": report.outbound
        })),
    )
}

/// 链信息查询接口。
async fn chain_info_handler(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
    match with_chain(&state, |chain| {
        let latest = chain.latest_block()?;
        let info = ChainInfoResponse {
            chain_id: chain.chain_id.clone(),
            height: latest.index,
            latest_hash: latest.hash.clone(),
            difficulty: chain.difficulty,
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

/// 按高度查询区块接口。
async fn chain_block_by_height_handler(
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
async fn chain_latest_block_handler(
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
async fn chain_validate_handler(
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
async fn chain_blocks_handler(
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
async fn chain_mempool_handler(
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

    match with_chain(&state, |chain| {
        let total = chain.pending_transactions.len();
        let filter_address = query.address.as_ref().map(|address| address.trim());
        let filtered = chain
            .pending_transactions
            .iter()
            .filter(|tx| match filter_address {
                Some(address) => tx.from == address || tx.to == address,
                None => true,
            })
            .cloned()
            .collect::<Vec<_>>();
        let matched_count = filtered.len();
        let transactions = match query.limit {
            Some(limit) => filtered.into_iter().take(limit).collect(),
            None => filtered,
        };
        Ok((total, matched_count, transactions))
    }) {
        Ok((total, matched_count, transactions)) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "total_pending_tx_count": total,
                "matched_count": matched_count,
                "filter_address": query.address,
                "returned_count": transactions.len(),
                "transactions": transactions
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// 按交易 ID 查询待打包交易详情。
async fn chain_pending_tx_handler(
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
async fn chain_address_txs_handler(
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
                if direction == "in" && tx_direction == "self" {
                    // self 方向同时属于 in/out，避免 in/out 过滤时遗漏。
                    // 此处保持记录方向为 self，便于调用方识别。
                }
                if direction == "out" && tx_direction == "self" {
                    // 同上，self 在 out 过滤下也保留。
                }

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
        let returned = if records.len() > limit {
            records.into_iter().take(limit).collect::<Vec<_>>()
        } else {
            records
        };
        Ok((total, returned))
    }) {
        Ok((total, transactions)) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "address": address,
                "direction": direction,
                "total_count": total,
                "returned_count": transactions.len(),
                "transactions": transactions
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// 按地址查询待打包交易列表（from/to 任一匹配）。
async fn chain_address_pending_txs_handler(
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
        let returned = if records.len() > limit {
            records.into_iter().take(limit).collect::<Vec<_>>()
        } else {
            records
        };
        Ok((total, returned))
    }) {
        Ok((total, transactions)) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "address": address,
                "direction": direction,
                "total_count": total,
                "returned_count": transactions.len(),
                "transactions": transactions
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// 地址汇总查询：返回余额与已确认/待打包收支统计。
async fn chain_address_summary_handler(
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
        for block in &chain.chain {
            for tx in &block.transactions {
                if tx.to == address {
                    confirmed_in_count = confirmed_in_count.saturating_add(1);
                }
                if tx.from == address {
                    confirmed_out_count = confirmed_out_count.saturating_add(1);
                }
            }
        }

        let mut pending_in_count = 0usize;
        let mut pending_out_count = 0usize;
        for tx in &chain.pending_transactions {
            if tx.to == address {
                pending_in_count = pending_in_count.saturating_add(1);
            }
            if tx.from == address {
                pending_out_count = pending_out_count.saturating_add(1);
            }
        }

        Ok((
            confirmed_in_count,
            confirmed_out_count,
            pending_in_count,
            pending_out_count,
        ))
    }) {
        Ok((confirmed_in_count, confirmed_out_count, pending_in_count, pending_out_count)) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "address": address,
                "balance": balance,
                "confirmed_in_count": confirmed_in_count,
                "confirmed_out_count": confirmed_out_count,
                "pending_in_count": pending_in_count,
                "pending_out_count": pending_out_count
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// 统一交易查询接口：优先查询待打包交易，其次查询历史交易。
async fn chain_tx_query_handler(
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
async fn chain_balance_handler(
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
async fn chain_contract_state_handler(
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
async fn chain_contract_events_handler(
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
async fn chain_contract_field_handler(
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
async fn chain_submit_tx_handler(
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
async fn chain_mine_handler(
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
async fn history_block_handler(
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
        Ok(Some(raw)) => match bincode::deserialize::<Block>(&raw) {
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
async fn history_tx_handler(
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

/// VM 编译接口：将源码编译为指令序列。
async fn vm_compile_handler(
    Json(payload): Json<VmCompileRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if payload.source.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "source 不能为空"
            })),
        );
    }

    match compile(&payload.source) {
        Ok(program) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "instruction_count": program.len(),
                "program": program
            })),
        ),
        Err(error) => {
            let (status, body) = map_vm_compile_error(error);
            (status, Json(body))
        }
    }
}

/// VM 执行接口：编译源码并执行，返回状态与事件。
async fn vm_execute_handler(
    Json(payload): Json<VmExecuteRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if payload.source.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "source 不能为空"
            })),
        );
    }

    let max_steps = payload.max_steps.unwrap_or(10_000);
    if max_steps == 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "max_steps 必须大于 0"
            })),
        );
    }

    let program = match compile(&payload.source) {
        Ok(program) => program,
        Err(error) => {
            let (status, body) = map_vm_compile_error(error);
            return (status, Json(body));
        }
    };

    let mut runtime = Runtime::default();
    match runtime.execute_with_limit(&program, max_steps) {
        Ok(report) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "instruction_count": program.len(),
                "report": {
                    "halted": report.halted,
                    "steps_executed": report.steps_executed,
                    "final_pc": report.final_pc
                },
                "state": runtime.state(),
                "events": runtime.events()
            })),
        ),
        Err(error) => {
            let (status, body) = map_vm_runtime_error(error);
            (status, Json(body))
        }
    }
}

/// DeFi 请求载荷。
#[derive(Debug, Deserialize)]
struct DefiActionRequest {
    /// 用户地址。
    owner: String,
    /// 操作数量。
    amount: u64,
}

/// DeFi 清算请求载荷。
#[derive(Debug, Deserialize)]
struct DefiLiquidateRequest {
    /// 被清算地址。
    borrower: String,
    /// 偿还债务数量。
    amount: u64,
}

/// DeFi 仓位响应。
#[derive(Debug, Serialize)]
struct DefiPositionResponse {
    /// 仓位用户。
    owner: String,
    /// 抵押数量。
    collateral_amount: u64,
    /// 借款数量。
    debt_amount: u64,
    /// 抵押率（bps）。
    collateral_ratio_bps: u64,
}

/// DeFi 池统计响应。
#[derive(Debug, Serialize)]
struct DefiPoolStatsResponse {
    /// 总抵押。
    total_collateral: u64,
    /// 总债务。
    total_debt: u64,
    /// 当前借款年化利率（bps）。
    borrow_rate_bps: u64,
    /// 已开仓位数量。
    position_count: usize,
}

/// DeFi 抵押接口。
async fn defi_deposit_handler(
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
async fn defi_borrow_handler(
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
async fn defi_repay_handler(
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
async fn defi_withdraw_handler(
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
async fn defi_liquidate_handler(
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
async fn defi_position_handler(
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
async fn defi_stats_handler(
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

/// NFT 铸造请求。
#[derive(Debug, Deserialize)]
struct NftMintRequest {
    /// 初始持有人地址。
    owner: String,
    /// NFT 名称。
    name: String,
    /// NFT 描述。
    description: String,
    /// 图片链接。
    image_url: String,
}

/// NFT 挂单请求。
#[derive(Debug, Deserialize)]
struct NftListRequest {
    /// 卖家地址。
    seller: String,
    /// NFT 资产 ID。
    token_id: String,
    /// 标价。
    price: u64,
}

/// NFT 取消挂单请求。
#[derive(Debug, Deserialize)]
struct NftCancelRequest {
    /// 卖家地址。
    seller: String,
    /// 挂单 ID。
    listing_id: String,
}

/// NFT 购买请求。
#[derive(Debug, Deserialize)]
struct NftBuyRequest {
    /// 买家地址。
    buyer: String,
    /// 挂单 ID。
    listing_id: String,
}

/// NFT 铸造接口。
async fn nft_mint_handler(
    State(state): State<AppState>,
    Json(payload): Json<NftMintRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    match with_market_mut(&state, |market| {
        let token = market.mint(
            &payload.owner,
            &payload.name,
            &payload.description,
            &payload.image_url,
        )?;
        Ok(json!({
            "ok": true,
            "token": token
        }))
    }) {
        Ok(body) => (StatusCode::OK, Json(body)),
        Err((status, body)) => (status, Json(body)),
    }
}

/// NFT 挂单接口。
async fn nft_list_handler(
    State(state): State<AppState>,
    Json(payload): Json<NftListRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    match with_market_mut(&state, |market| {
        let listing = market.list(&payload.seller, &payload.token_id, payload.price)?;
        Ok(json!({
            "ok": true,
            "listing": listing
        }))
    }) {
        Ok(body) => (StatusCode::OK, Json(body)),
        Err((status, body)) => (status, Json(body)),
    }
}

/// NFT 取消挂单接口。
async fn nft_cancel_handler(
    State(state): State<AppState>,
    Json(payload): Json<NftCancelRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    match with_market_mut(&state, |market| {
        let listing = market.cancel_listing(&payload.seller, &payload.listing_id)?;
        Ok(json!({
            "ok": true,
            "listing": listing
        }))
    }) {
        Ok(body) => (StatusCode::OK, Json(body)),
        Err((status, body)) => (status, Json(body)),
    }
}

/// NFT 购买接口。
async fn nft_buy_handler(
    State(state): State<AppState>,
    Json(payload): Json<NftBuyRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    match with_market_mut(&state, |market| {
        let outcome = market.buy(&payload.buyer, &payload.listing_id)?;
        Ok(json!({
            "ok": true,
            "outcome": outcome
        }))
    }) {
        Ok(body) => (StatusCode::OK, Json(body)),
        Err((status, body)) => (status, Json(body)),
    }
}

/// NFT 查询资产接口。
async fn nft_token_handler(
    State(state): State<AppState>,
    Path(token_id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    match with_market(&state, |market| {
        let token =
            market
                .tokens
                .get(&token_id)
                .cloned()
                .ok_or_else(|| NftError::TokenNotFound {
                    token_id: token_id.clone(),
                })?;
        Ok(json!({
            "ok": true,
            "token": token
        }))
    }) {
        Ok(body) => (StatusCode::OK, Json(body)),
        Err((status, body)) => (status, Json(body)),
    }
}

/// NFT 查询挂单接口。
async fn nft_listing_handler(
    State(state): State<AppState>,
    Path(listing_id): Path<String>,
) -> (StatusCode, Json<serde_json::Value>) {
    match with_market(&state, |market| {
        let listing =
            market
                .listings
                .get(&listing_id)
                .cloned()
                .ok_or_else(|| NftError::ListingNotFound {
                    listing_id: listing_id.clone(),
                })?;
        Ok(json!({
            "ok": true,
            "listing": listing
        }))
    }) {
        Ok(body) => (StatusCode::OK, Json(body)),
        Err((status, body)) => (status, Json(body)),
    }
}

/// NFT 查询活跃挂单接口。
async fn nft_active_listings_handler(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
    match with_market(&state, |market| {
        let active_listings: Vec<_> = market
            .listings
            .values()
            .filter(|listing| listing.status == rustchain_apps::nft::ListingStatus::Active)
            .cloned()
            .collect();
        Ok(json!({
            "ok": true,
            "listings": active_listings
        }))
    }) {
        Ok(body) => (StatusCode::OK, Json(body)),
        Err((status, body)) => (status, Json(body)),
    }
}

/// NFT 查询用户资产接口。
async fn nft_owner_tokens_handler(
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

    match with_market(&state, |market| {
        let tokens: Vec<_> = market
            .tokens
            .values()
            .filter(|token| token.owner == owner)
            .cloned()
            .collect();
        Ok(json!({
            "ok": true,
            "tokens": tokens
        }))
    }) {
        Ok(body) => (StatusCode::OK, Json(body)),
        Err((status, body)) => (status, Json(body)),
    }
}

/// 只读访问区块链状态。
fn with_chain<T>(
    state: &AppState,
    f: impl FnOnce(&Blockchain) -> Result<T, rustchain_core::error::CoreError>,
) -> Result<T, (StatusCode, serde_json::Value)> {
    let guard = state.blockchain.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({
                "ok": false,
                "error": "blockchain 锁异常"
            }),
        )
    })?;

    f(&guard).map_err(map_core_error)
}

/// 可变访问区块链状态。
fn with_chain_mut<T>(
    state: &AppState,
    f: impl FnOnce(&mut Blockchain) -> Result<T, rustchain_core::error::CoreError>,
) -> Result<T, (StatusCode, serde_json::Value)> {
    let mut guard = state.blockchain.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({
                "ok": false,
                "error": "blockchain 锁异常"
            }),
        )
    })?;

    f(&mut guard).map_err(map_core_error)
}

/// 只读访问 P2P 引擎。
fn with_p2p<T>(
    state: &AppState,
    f: impl FnOnce(&SyncEngine) -> Result<T, rustchain_p2p::P2pError>,
) -> Result<T, (StatusCode, serde_json::Value)> {
    let guard = state.p2p_engine.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({
                "ok": false,
                "error": "p2p_engine 锁异常"
            }),
        )
    })?;

    f(&guard).map_err(map_p2p_error)
}

/// 可变访问 P2P 引擎。
fn with_p2p_mut<T>(
    state: &AppState,
    f: impl FnOnce(&mut SyncEngine) -> Result<T, rustchain_p2p::P2pError>,
) -> Result<T, (StatusCode, serde_json::Value)> {
    let mut guard = state.p2p_engine.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({
                "ok": false,
                "error": "p2p_engine 锁异常"
            }),
        )
    })?;

    f(&mut guard).map_err(map_p2p_error)
}

/// 只读访问借贷池。
fn with_pool<T>(
    state: &AppState,
    f: impl FnOnce(&LendingPool) -> Result<T, DefiError>,
) -> Result<T, (StatusCode, serde_json::Value)> {
    let guard = state.lending_pool.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({
                "ok": false,
                "error": "lending_pool 锁异常"
            }),
        )
    })?;

    f(&guard).map_err(map_defi_error)
}

/// 可变访问借贷池。
fn with_pool_mut<T>(
    state: &AppState,
    f: impl FnOnce(&mut LendingPool) -> Result<T, DefiError>,
) -> Result<T, (StatusCode, serde_json::Value)> {
    let mut guard = state.lending_pool.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({
                "ok": false,
                "error": "lending_pool 锁异常"
            }),
        )
    })?;

    f(&mut guard).map_err(map_defi_error)
}

/// 只读访问 NFT 市场。
fn with_market<T>(
    state: &AppState,
    f: impl FnOnce(&NftMarketplace) -> Result<T, NftError>,
) -> Result<T, (StatusCode, serde_json::Value)> {
    let guard = state.nft_marketplace.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({
                "ok": false,
                "error": "nft_marketplace 锁异常"
            }),
        )
    })?;

    f(&guard).map_err(map_nft_error)
}

/// 可变访问 NFT 市场。
fn with_market_mut<T>(
    state: &AppState,
    f: impl FnOnce(&mut NftMarketplace) -> Result<T, NftError>,
) -> Result<T, (StatusCode, serde_json::Value)> {
    let mut guard = state.nft_marketplace.lock().map_err(|_| {
        (
            StatusCode::INTERNAL_SERVER_ERROR,
            json!({
                "ok": false,
                "error": "nft_marketplace 锁异常"
            }),
        )
    })?;

    f(&mut guard).map_err(map_nft_error)
}

/// 访问历史存储。
fn with_history<T>(
    state: &AppState,
    f: impl FnOnce(&dyn HistoryStore) -> Result<T, StorageError>,
) -> Result<T, (StatusCode, serde_json::Value)> {
    f(state.history_store.as_ref()).map_err(map_storage_error)
}

/// 访问状态存储。
fn with_state_store<T>(
    state: &AppState,
    f: impl FnOnce(&dyn StateStore) -> Result<T, StorageError>,
) -> Result<T, (StatusCode, serde_json::Value)> {
    f(state.state_store.as_ref()).map_err(map_storage_error)
}

/// 持久化新挖出的区块及其交易历史。
fn persist_mined_block(history: &dyn HistoryStore, block: &Block) -> Result<(), StorageError> {
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
fn persist_runtime_state(
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
fn encode_blocks(blocks: Vec<Block>) -> Result<Vec<Vec<u8>>, bincode::Error> {
    blocks
        .into_iter()
        .map(|block| bincode::serialize(&block))
        .collect()
}

/// 编码交易列表为网络传输字节。
fn encode_transactions(transactions: Vec<Transaction>) -> Result<Vec<Vec<u8>>, bincode::Error> {
    transactions
        .into_iter()
        .map(|tx| bincode::serialize(&tx))
        .collect()
}

/// 接收同步区块并完成链、历史、状态和 P2P 摘要更新。
fn accept_synced_block(
    state: &AppState,
    block: Block,
) -> Result<(), (StatusCode, serde_json::Value)> {
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
fn build_next_get_blocks_request(
    state: &AppState,
    peer_id: &str,
) -> Result<Option<NetworkMessage>, (StatusCode, serde_json::Value)> {
    let local_height = with_chain(state, |chain| Ok(chain.latest_block()?.index))?;
    let peer_best_height = with_p2p(state, |engine| {
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
fn decode_i64_from_le_bytes(raw: &[u8]) -> Option<i64> {
    if raw.len() != 8 {
        return None;
    }

    let mut bytes = [0u8; 8];
    bytes.copy_from_slice(raw);
    Some(i64::from_le_bytes(bytes))
}

/// DeFi 业务错误映射为 HTTP 错误响应。
fn map_defi_error(error: DefiError) -> (StatusCode, serde_json::Value) {
    let status = match error {
        DefiError::ArithmeticOverflow => StatusCode::INTERNAL_SERVER_ERROR,
        _ => StatusCode::BAD_REQUEST,
    };
    (
        status,
        json!({
            "ok": false,
            "error": error.to_string()
        }),
    )
}

/// NFT 业务错误映射为 HTTP 错误响应。
fn map_nft_error(error: NftError) -> (StatusCode, serde_json::Value) {
    let status = match error {
        NftError::TokenNotFound { .. } | NftError::ListingNotFound { .. } => StatusCode::NOT_FOUND,
        _ => StatusCode::BAD_REQUEST,
    };
    (
        status,
        json!({
            "ok": false,
            "error": error.to_string()
        }),
    )
}

/// 核心链错误映射为 HTTP 错误响应。
fn map_core_error(error: rustchain_core::error::CoreError) -> (StatusCode, serde_json::Value) {
    let status = match error {
        rustchain_core::error::CoreError::EmptyChain
        | rustchain_core::error::CoreError::InvalidGenesisBlock
        | rustchain_core::error::CoreError::InvalidBlockHash { .. }
        | rustchain_core::error::CoreError::InvalidPreviousHash { .. }
        | rustchain_core::error::CoreError::InvalidMerkleRoot { .. }
        | rustchain_core::error::CoreError::InvalidProofOfWork { .. }
        | rustchain_core::error::CoreError::InvalidBlockDifficulty { .. }
        | rustchain_core::error::CoreError::InvalidBlockIndex { .. } => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
        _ => StatusCode::BAD_REQUEST,
    };
    (
        status,
        json!({
            "ok": false,
            "error": error.to_string()
        }),
    )
}

/// 存储错误映射为 HTTP 错误响应。
fn map_storage_error(error: StorageError) -> (StatusCode, serde_json::Value) {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({
            "ok": false,
            "error": error.to_string()
        }),
    )
}

/// P2P 错误映射为 HTTP 错误响应。
fn map_p2p_error(error: rustchain_p2p::P2pError) -> (StatusCode, serde_json::Value) {
    let status = match error {
        rustchain_p2p::P2pError::InvalidArgument(_)
        | rustchain_p2p::P2pError::InvalidMessage(_)
        | rustchain_p2p::P2pError::StaleSequence { .. } => StatusCode::BAD_REQUEST,
        rustchain_p2p::P2pError::Serialize(_) | rustchain_p2p::P2pError::Deserialize(_) => {
            StatusCode::INTERNAL_SERVER_ERROR
        }
    };
    (
        status,
        json!({
            "ok": false,
            "error": error.to_string()
        }),
    )
}

/// VM 编译错误映射为 HTTP 错误响应。
fn map_vm_compile_error(error: CompileError) -> (StatusCode, serde_json::Value) {
    (
        StatusCode::BAD_REQUEST,
        json!({
            "ok": false,
            "error": error.to_string()
        }),
    )
}

/// VM 运行时错误映射为 HTTP 错误响应。
fn map_vm_runtime_error(error: VmError) -> (StatusCode, serde_json::Value) {
    (
        StatusCode::BAD_REQUEST,
        json!({
            "ok": false,
            "error": error.to_string()
        }),
    )
}

/// 获取当前 Unix 秒级时间戳。
fn now_unix_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::{build_app, default_app_state};
    use axum::{
        body::{to_bytes, Body},
        http::{Method, Request, StatusCode},
        Router,
    };
    use rustchain_core::block::Block;
    use rustchain_core::blockchain::Blockchain;
    use rustchain_core::transaction::{Transaction, TransactionKind};
    use rustchain_crypto::wallet::create_wallet;
    use serde_json::{json, Value};
    use tower::ServiceExt;

    /// 验证链信息、交易提交、挖矿和余额查询主流程。
    #[tokio::test]
    async fn chain_flow_should_work() {
        let app = build_test_app();
        let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
        let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

        let (status, body) = send_empty(&app, Method::GET, "/chain/info").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["chain"]["height"], json!(0));

        let (status, _) = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": alice_wallet.address.clone() }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let mut tx = Transaction::new(
            alice_wallet.address.clone(),
            bob_wallet.address.clone(),
            20,
            Some(b"api-chain-flow".to_vec()),
        );
        tx.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
            .expect("签名应成功");
        let tx_id = tx.id.clone();

        let (status, body) = send_json(
            &app,
            Method::POST,
            "/chain/tx",
            json!({ "transaction": tx }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["pending_tx_count"], json!(1));

        let (status, body) = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": "miner-2" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let block_hash = body["block"]["hash"]
            .as_str()
            .expect("block.hash 应存在")
            .to_string();

        let (status, body) =
            send_empty(&app, Method::GET, &format!("/history/block/{block_hash}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["block"]["hash"], json!(block_hash));

        let (status, body) = send_empty(&app, Method::GET, &format!("/history/tx/{tx_id}")).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["transaction"]["id"], json!(tx_id));

        let (status, body) = send_empty(
            &app,
            Method::GET,
            &format!("/chain/balance/{}", alice_wallet.address),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["balance"], json!(30));

        let (status, body) = send_empty(
            &app,
            Method::GET,
            &format!("/chain/balance/{}", bob_wallet.address),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["balance"], json!(20));
    }

    /// 验证交易池查询支持返回完整列表和 limit 截断。
    #[tokio::test]
    async fn chain_mempool_should_support_limit_query() {
        let app = build_test_app();
        let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
        let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": alice_wallet.address.clone() }),
        )
        .await;

        let mut tx1 = Transaction::new(
            alice_wallet.address.clone(),
            bob_wallet.address.clone(),
            10,
            Some(b"mempool-test-1".to_vec()),
        );
        tx1.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
            .expect("签名应成功");
        let tx1_id = tx1.id.clone();

        let mut tx2 = Transaction::new(
            alice_wallet.address.clone(),
            bob_wallet.address.clone(),
            5,
            Some(b"mempool-test-2".to_vec()),
        );
        tx2.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
            .expect("签名应成功");
        let tx2_id = tx2.id.clone();

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/tx",
            json!({ "transaction": tx1 }),
        )
        .await;
        let _ = send_json(
            &app,
            Method::POST,
            "/chain/tx",
            json!({ "transaction": tx2 }),
        )
        .await;

        let (status, body) = send_empty(&app, Method::GET, "/chain/mempool").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["total_pending_tx_count"], json!(2));
        assert_eq!(body["returned_count"], json!(2));

        let all_txs = body["transactions"]
            .as_array()
            .expect("transactions 应为数组");
        assert_eq!(all_txs.len(), 2);
        let all_ids = all_txs
            .iter()
            .filter_map(|tx| tx["id"].as_str())
            .collect::<Vec<_>>();
        assert!(all_ids.contains(&tx1_id.as_str()));
        assert!(all_ids.contains(&tx2_id.as_str()));

        let (status, body) = send_empty(&app, Method::GET, "/chain/mempool?limit=1").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["total_pending_tx_count"], json!(2));
        assert_eq!(body["returned_count"], json!(1));
        assert_eq!(
            body["transactions"]
                .as_array()
                .expect("transactions 应为数组")
                .len(),
            1
        );
    }

    /// 验证交易池查询 limit=0 会被拒绝。
    #[tokio::test]
    async fn chain_mempool_with_zero_limit_should_fail() {
        let app = build_test_app();
        let (status, body) = send_empty(&app, Method::GET, "/chain/mempool?limit=0").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["ok"], json!(false));
    }

    /// 验证交易池查询支持按地址过滤（from/to 任一匹配）。
    #[tokio::test]
    async fn chain_mempool_should_support_address_filter() {
        let app = build_test_app();
        let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
        let (carol_wallet, carol_key_pair) = create_wallet("carol-pass").expect("创建钱包应成功");
        let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": alice_wallet.address.clone() }),
        )
        .await;
        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": carol_wallet.address.clone() }),
        )
        .await;

        let mut tx1 = Transaction::new(
            alice_wallet.address.clone(),
            bob_wallet.address.clone(),
            8,
            Some(b"mempool-filter-alice".to_vec()),
        );
        tx1.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
            .expect("签名应成功");
        let tx1_id = tx1.id.clone();

        let mut tx2 = Transaction::new(
            carol_wallet.address.clone(),
            bob_wallet.address.clone(),
            9,
            Some(b"mempool-filter-carol".to_vec()),
        );
        tx2.sign_with_private_key(&carol_key_pair.private_key, &carol_key_pair.public_key)
            .expect("签名应成功");

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/tx",
            json!({ "transaction": tx1 }),
        )
        .await;
        let _ = send_json(
            &app,
            Method::POST,
            "/chain/tx",
            json!({ "transaction": tx2 }),
        )
        .await;

        let path = format!("/chain/mempool?address={}", alice_wallet.address);
        let (status, body) = send_empty(&app, Method::GET, &path).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["total_pending_tx_count"], json!(2));
        assert_eq!(body["matched_count"], json!(1));
        assert_eq!(body["returned_count"], json!(1));
        assert_eq!(body["transactions"][0]["id"], json!(tx1_id));
    }

    /// 验证交易池地址过滤参数不能为空字符串。
    #[tokio::test]
    async fn chain_mempool_with_empty_address_should_fail() {
        let app = build_test_app();
        let (status, body) = send_empty(&app, Method::GET, "/chain/mempool?address=").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["ok"], json!(false));
    }

    /// 验证可按 tx_id 查询待打包交易详情。
    #[tokio::test]
    async fn chain_pending_tx_should_return_transaction() {
        let app = build_test_app();
        let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
        let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": alice_wallet.address.clone() }),
        )
        .await;

        let mut tx = Transaction::new(
            alice_wallet.address.clone(),
            bob_wallet.address.clone(),
            11,
            Some(b"pending-tx-query".to_vec()),
        );
        tx.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
            .expect("签名应成功");
        let tx_id = tx.id.clone();

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/tx",
            json!({ "transaction": tx }),
        )
        .await;

        let path = format!("/chain/pending-tx/{tx_id}");
        let (status, body) = send_empty(&app, Method::GET, &path).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], json!(true));
        assert_eq!(body["transaction"]["id"], json!(tx_id));
    }

    /// 验证查询不存在的待打包交易返回 404。
    #[tokio::test]
    async fn chain_pending_tx_not_found_should_return_404() {
        let app = build_test_app();
        let (status, body) = send_empty(&app, Method::GET, "/chain/pending-tx/tx-missing").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["ok"], json!(false));
    }

    /// 验证可按地址查询已确认交易列表。
    #[tokio::test]
    async fn chain_address_txs_should_return_confirmed_transactions() {
        let app = build_test_app();
        let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
        let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": alice_wallet.address.clone() }),
        )
        .await;

        let mut tx = Transaction::new(
            alice_wallet.address.clone(),
            bob_wallet.address.clone(),
            12,
            Some(b"address-txs-query".to_vec()),
        );
        tx.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
            .expect("签名应成功");
        let tx_id = tx.id.clone();

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/tx",
            json!({ "transaction": tx }),
        )
        .await;
        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": "collector-miner" }),
        )
        .await;

        let path = format!("/chain/address/{}/txs", bob_wallet.address);
        let (status, body) = send_empty(&app, Method::GET, &path).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], json!(true));
        assert_eq!(body["returned_count"], json!(1));
        assert_eq!(body["transactions"][0]["transaction"]["id"], json!(tx_id));
    }

    /// 验证地址交易查询 limit=0 会被拒绝。
    #[tokio::test]
    async fn chain_address_txs_with_zero_limit_should_fail() {
        let app = build_test_app();
        let (status, body) =
            send_empty(&app, Method::GET, "/chain/address/alice/txs?limit=0").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["ok"], json!(false));
    }

    /// 验证地址交易查询支持方向过滤（in/out）。
    #[tokio::test]
    async fn chain_address_txs_should_support_direction_filter() {
        let app = build_test_app();
        let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
        let (bob_wallet, bob_key_pair) = create_wallet("bob-pass").expect("创建钱包应成功");
        let (carol_wallet, _) = create_wallet("carol-pass").expect("创建钱包应成功");

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": alice_wallet.address.clone() }),
        )
        .await;

        let mut tx_in = Transaction::new(
            alice_wallet.address.clone(),
            bob_wallet.address.clone(),
            15,
            Some(b"address-txs-direction-in".to_vec()),
        );
        tx_in
            .sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
            .expect("签名应成功");
        let tx_in_id = tx_in.id.clone();

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/tx",
            json!({ "transaction": tx_in }),
        )
        .await;
        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": "direction-miner-a" }),
        )
        .await;

        let mut tx_out = Transaction::new(
            bob_wallet.address.clone(),
            carol_wallet.address.clone(),
            6,
            Some(b"address-txs-direction-out".to_vec()),
        );
        tx_out
            .sign_with_private_key(&bob_key_pair.private_key, &bob_key_pair.public_key)
            .expect("签名应成功");
        let tx_out_id = tx_out.id.clone();

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/tx",
            json!({ "transaction": tx_out }),
        )
        .await;
        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": "direction-miner-b" }),
        )
        .await;

        let path_in = format!("/chain/address/{}/txs?direction=in", bob_wallet.address);
        let (status, body) = send_empty(&app, Method::GET, &path_in).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["returned_count"], json!(1));
        assert_eq!(
            body["transactions"][0]["transaction"]["id"],
            json!(tx_in_id)
        );

        let path_out = format!("/chain/address/{}/txs?direction=out", bob_wallet.address);
        let (status, body) = send_empty(&app, Method::GET, &path_out).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["returned_count"], json!(1));
        assert_eq!(
            body["transactions"][0]["transaction"]["id"],
            json!(tx_out_id)
        );
    }

    /// 验证地址交易查询传入非法 direction 会被拒绝。
    #[tokio::test]
    async fn chain_address_txs_with_invalid_direction_should_fail() {
        let app = build_test_app();
        let (status, body) = send_empty(
            &app,
            Method::GET,
            "/chain/address/alice/txs?direction=sideways",
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["ok"], json!(false));
    }

    /// 验证可按地址查询待打包交易列表。
    #[tokio::test]
    async fn chain_address_pending_txs_should_return_transactions() {
        let app = build_test_app();
        let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
        let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": alice_wallet.address.clone() }),
        )
        .await;

        let mut tx = Transaction::new(
            alice_wallet.address.clone(),
            bob_wallet.address.clone(),
            9,
            Some(b"address-pending-txs-query".to_vec()),
        );
        tx.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
            .expect("签名应成功");
        let tx_id = tx.id.clone();

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/tx",
            json!({ "transaction": tx }),
        )
        .await;

        let path = format!(
            "/chain/address/{}/pending-txs?direction=in",
            bob_wallet.address
        );
        let (status, body) = send_empty(&app, Method::GET, &path).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], json!(true));
        assert_eq!(body["returned_count"], json!(1));
        assert_eq!(body["transactions"][0]["transaction"]["id"], json!(tx_id));
    }

    /// 验证待打包交易地址查询传入非法 direction 会被拒绝。
    #[tokio::test]
    async fn chain_address_pending_txs_with_invalid_direction_should_fail() {
        let app = build_test_app();
        let (status, body) = send_empty(
            &app,
            Method::GET,
            "/chain/address/alice/pending-txs?direction=sideways",
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["ok"], json!(false));
    }

    /// 验证可查询地址汇总（余额 + 已确认/待打包收支统计）。
    #[tokio::test]
    async fn chain_address_summary_should_return_counts_and_balance() {
        let app = build_test_app();
        let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
        let (bob_wallet, bob_key_pair) = create_wallet("bob-pass").expect("创建钱包应成功");

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": alice_wallet.address.clone() }),
        )
        .await;

        let mut tx_confirmed = Transaction::new(
            alice_wallet.address.clone(),
            bob_wallet.address.clone(),
            7,
            Some(b"address-summary-confirmed".to_vec()),
        );
        tx_confirmed
            .sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
            .expect("签名应成功");
        let _ = send_json(
            &app,
            Method::POST,
            "/chain/tx",
            json!({ "transaction": tx_confirmed }),
        )
        .await;
        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": "summary-miner" }),
        )
        .await;

        let mut tx_pending = Transaction::new(
            bob_wallet.address.clone(),
            alice_wallet.address.clone(),
            3,
            Some(b"address-summary-pending".to_vec()),
        );
        tx_pending
            .sign_with_private_key(&bob_key_pair.private_key, &bob_key_pair.public_key)
            .expect("签名应成功");
        let _ = send_json(
            &app,
            Method::POST,
            "/chain/tx",
            json!({ "transaction": tx_pending }),
        )
        .await;

        let path = format!("/chain/address/{}/summary", bob_wallet.address);
        let (status, body) = send_empty(&app, Method::GET, &path).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], json!(true));
        assert_eq!(body["balance"], json!(7));
        assert_eq!(body["confirmed_in_count"], json!(1));
        assert_eq!(body["confirmed_out_count"], json!(0));
        assert_eq!(body["pending_in_count"], json!(0));
        assert_eq!(body["pending_out_count"], json!(1));
    }

    /// 验证可按高度查询区块详情。
    #[tokio::test]
    async fn chain_block_by_height_should_return_block() {
        let app = build_test_app();

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": "miner-by-height" }),
        )
        .await;

        let (status, body) = send_empty(&app, Method::GET, "/chain/block/1").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], json!(true));
        assert_eq!(body["block"]["index"], json!(1));
    }

    /// 验证查询不存在高度时返回 404。
    #[tokio::test]
    async fn chain_block_by_height_not_found_should_return_404() {
        let app = build_test_app();
        let (status, body) = send_empty(&app, Method::GET, "/chain/block/99").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["ok"], json!(false));
    }

    /// 验证可按区间查询区块列表。
    #[tokio::test]
    async fn chain_blocks_should_support_from_height_and_limit() {
        let app = build_test_app();

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": "miner-range-1" }),
        )
        .await;
        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": "miner-range-2" }),
        )
        .await;
        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": "miner-range-3" }),
        )
        .await;

        let (status, body) =
            send_empty(&app, Method::GET, "/chain/blocks?from_height=1&limit=2").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], json!(true));
        assert_eq!(body["returned_count"], json!(2));

        let blocks = body["blocks"].as_array().expect("blocks 应为数组");
        assert_eq!(blocks.len(), 2);
        assert_eq!(blocks[0]["index"], json!(1));
        assert_eq!(blocks[1]["index"], json!(2));
    }

    /// 验证区块列表查询 limit=0 会被拒绝。
    #[tokio::test]
    async fn chain_blocks_with_zero_limit_should_fail() {
        let app = build_test_app();
        let (status, body) = send_empty(&app, Method::GET, "/chain/blocks?limit=0").await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["ok"], json!(false));
    }

    /// 验证可查询最新区块详情。
    #[tokio::test]
    async fn chain_latest_block_should_return_latest() {
        let app = build_test_app();

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": "latest-miner-a" }),
        )
        .await;
        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": "latest-miner-b" }),
        )
        .await;

        let (status, body) = send_empty(&app, Method::GET, "/chain/block/latest").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], json!(true));
        assert_eq!(body["block"]["index"], json!(2));
    }

    /// 验证链完整性校验接口可返回通过结果。
    #[tokio::test]
    async fn chain_validate_should_return_valid() {
        let app = build_test_app();

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": "validate-miner" }),
        )
        .await;

        let (status, body) = send_empty(&app, Method::GET, "/chain/validate").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], json!(true));
        assert_eq!(body["valid"], json!(true));
    }

    /// 验证 P2P 节点注册与 Ping/Pong 消息处理流程。
    #[tokio::test]
    async fn p2p_register_and_ping_should_work() {
        let app = build_test_app();

        let (status, body) = send_json(
            &app,
            Method::POST,
            "/p2p/peer/register",
            json!({
                "peer_id": "peer-a",
                "address": "/ip4/127.0.0.1/tcp/7001"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["peer_count"], json!(1));

        let (status, body) = send_empty(&app, Method::GET, "/p2p/status").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["peer_count"], json!(1));

        let (status, body) = send_json(
            &app,
            Method::POST,
            "/p2p/message",
            json!({
                "peer_id": "peer-a",
                "address": "/ip4/127.0.0.1/tcp/7001",
                "sequence": 1,
                "message": {
                    "Ping": {
                        "nonce": 7,
                        "timestamp": 99
                    }
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["processed"], json!(1));
        assert_eq!(body["outbound_count"], json!(1));
    }

    /// 验证链交易提交与挖矿会触发 P2P 广播。
    #[tokio::test]
    async fn chain_actions_should_broadcast_to_connected_peers() {
        let app = build_test_app();
        let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
        let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

        let _ = send_json(
            &app,
            Method::POST,
            "/p2p/peer/register",
            json!({
                "peer_id": "peer-a",
                "address": "/ip4/127.0.0.1/tcp/7001"
            }),
        )
        .await;
        let _ = send_json(
            &app,
            Method::POST,
            "/p2p/message",
            json!({
                "peer_id": "peer-a",
                "address": "/ip4/127.0.0.1/tcp/7001",
                "sequence": 1,
                "message": "GetChainStatus"
            }),
        )
        .await;

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": alice_wallet.address.clone() }),
        )
        .await;

        let mut tx = Transaction::new(
            alice_wallet.address.clone(),
            bob_wallet.address.clone(),
            10,
            Some(b"api-p2p-broadcast".to_vec()),
        );
        tx.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
            .expect("签名应成功");

        let (status, body) = send_json(
            &app,
            Method::POST,
            "/chain/tx",
            json!({ "transaction": tx }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["p2p_outbound_count"], json!(1));

        let (status, body) = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": "miner-2" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["p2p_outbound_count"], json!(1));
    }

    /// 验证 P2P GetBlocks 会返回真实区块数据。
    #[tokio::test]
    async fn p2p_get_blocks_should_return_real_blocks() {
        let app = build_test_app();
        let (miner_wallet, _) = create_wallet("miner-pass").expect("创建钱包应成功");

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": miner_wallet.address.clone() }),
        )
        .await;
        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": miner_wallet.address.clone() }),
        )
        .await;

        let (status, body) = send_json(
            &app,
            Method::POST,
            "/p2p/message",
            json!({
                "peer_id": "peer-a",
                "address": "/ip4/127.0.0.1/tcp/7001",
                "sequence": 1,
                "message": {
                    "GetBlocks": {
                        "from_height": 1,
                        "limit": 10
                    }
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["outbound_count"], json!(1));
        let blocks = body["outbound"][0]["message"]["Blocks"]["blocks"]
            .as_array()
            .expect("blocks 应为数组");
        assert_eq!(blocks.len(), 2);
    }

    /// 验证 P2P NewTransaction 会导入本地交易池。
    #[tokio::test]
    async fn p2p_new_transaction_should_import_to_mempool() {
        let app = build_test_app();
        let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
        let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": alice_wallet.address.clone() }),
        )
        .await;

        let mut tx = Transaction::new(
            alice_wallet.address.clone(),
            bob_wallet.address.clone(),
            5,
            Some(b"p2p-sync-tx".to_vec()),
        );
        tx.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
            .expect("签名应成功");
        let tx_bytes = bincode::serialize(&tx).expect("交易编码应成功");

        let (status, body) = send_json(
            &app,
            Method::POST,
            "/p2p/message",
            json!({
                "peer_id": "peer-a",
                "address": "/ip4/127.0.0.1/tcp/7001",
                "sequence": 1,
                "message": {
                    "NewTransaction": {
                        "transaction": tx_bytes
                    }
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], json!(true));

        let (status, body) = send_empty(&app, Method::GET, "/chain/info").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["chain"]["pending_tx_count"], json!(1));
    }

    /// 验证 P2P GetMempool 会返回真实交易池快照。
    #[tokio::test]
    async fn p2p_get_mempool_should_return_real_transactions() {
        let app = build_test_app();
        let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
        let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": alice_wallet.address.clone() }),
        )
        .await;

        let mut tx = Transaction::new(
            alice_wallet.address.clone(),
            bob_wallet.address.clone(),
            6,
            Some(b"p2p-get-mempool".to_vec()),
        );
        tx.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
            .expect("签名应成功");
        let tx_id = tx.id.clone();
        let _ = send_json(
            &app,
            Method::POST,
            "/chain/tx",
            json!({ "transaction": tx }),
        )
        .await;

        let (status, body) = send_json(
            &app,
            Method::POST,
            "/p2p/message",
            json!({
                "peer_id": "peer-a",
                "address": "/ip4/127.0.0.1/tcp/7001",
                "sequence": 1,
                "message": "GetMempool"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["outbound_count"], json!(1));

        let tx_list = body["outbound"][0]["message"]["Mempool"]["transactions"]
            .as_array()
            .expect("transactions 应为数组");
        assert_eq!(tx_list.len(), 1);

        let raw: Vec<u8> =
            serde_json::from_value(tx_list[0].clone()).expect("交易字节应可反序列化");
        let decoded_tx: Transaction = bincode::deserialize(&raw).expect("交易应可反序列化");
        assert_eq!(decoded_tx.id, tx_id);
    }

    /// 验证 P2P Mempool 会批量导入交易到本地交易池。
    #[tokio::test]
    async fn p2p_mempool_should_import_transactions() {
        let app = build_test_app();
        let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
        let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": alice_wallet.address.clone() }),
        )
        .await;

        let mut tx = Transaction::new(
            alice_wallet.address.clone(),
            bob_wallet.address.clone(),
            7,
            Some(b"p2p-import-mempool".to_vec()),
        );
        tx.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
            .expect("签名应成功");
        let tx_bytes = bincode::serialize(&tx).expect("交易编码应成功");

        let (status, body) = send_json(
            &app,
            Method::POST,
            "/p2p/message",
            json!({
                "peer_id": "peer-a",
                "address": "/ip4/127.0.0.1/tcp/7001",
                "sequence": 1,
                "message": {
                    "Mempool": {
                        "transactions": [tx_bytes]
                    }
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], json!(true));

        let (status, body) = send_empty(&app, Method::GET, "/chain/info").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["chain"]["pending_tx_count"], json!(1));
    }

    /// 验证 P2P NewBlock 会同步并追加本地区块。
    #[tokio::test]
    async fn p2p_new_block_should_sync_chain() {
        let app = build_test_app();
        let mut remote_chain = Blockchain::new(2, 50);
        let block = remote_chain
            .mine_pending_transactions("remote-miner")
            .expect("远端出块应成功");
        let block_bytes = bincode::serialize(&block).expect("区块编码应成功");

        let (status, body) = send_json(
            &app,
            Method::POST,
            "/p2p/message",
            json!({
                "peer_id": "peer-a",
                "address": "/ip4/127.0.0.1/tcp/7001",
                "sequence": 1,
                "message": {
                    "NewBlock": {
                        "block": block_bytes
                    }
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], json!(true));

        let (status, body) = send_empty(&app, Method::GET, "/chain/info").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["chain"]["height"], json!(1));
    }

    /// 验证接收 Blocks 后若仍落后，会继续请求下一批区块。
    #[tokio::test]
    async fn p2p_blocks_should_request_next_batch_when_still_behind() {
        let app = build_test_app();
        let mut remote_chain = Blockchain::new(2, 50);
        let block = remote_chain
            .mine_pending_transactions("remote-miner")
            .expect("远端出块应成功");
        let block_bytes = bincode::serialize(&block).expect("区块编码应成功");

        let (status, _) = send_json(
            &app,
            Method::POST,
            "/p2p/message",
            json!({
                "peer_id": "peer-a",
                "address": "/ip4/127.0.0.1/tcp/7001",
                "sequence": 1,
                "message": {
                    "ChainStatus": {
                        "chain_id": "rustchain-lab-dev",
                        "best_height": 5,
                        "best_hash": "remote-tip-5",
                        "difficulty": 2,
                        "genesis_hash": Block::genesis().hash
                    }
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = send_json(
            &app,
            Method::POST,
            "/p2p/message",
            json!({
                "peer_id": "peer-a",
                "address": "/ip4/127.0.0.1/tcp/7001",
                "sequence": 2,
                "message": {
                    "Blocks": {
                        "blocks": [block_bytes]
                    }
                }
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["outbound_count"], json!(1));
        assert_eq!(
            body["outbound"][0]["message"]["GetBlocks"]["from_height"],
            json!(2)
        );
        assert_eq!(
            body["outbound"][0]["message"]["GetBlocks"]["limit"],
            json!(4)
        );
    }

    /// 验证 DeFi 抵押、借款和仓位查询主流程。
    #[tokio::test]
    async fn defi_flow_should_work() {
        let app = build_test_app();

        let (status, _) = send_json(
            &app,
            Method::POST,
            "/defi/deposit",
            json!({
                "owner": "alice",
                "amount": 200
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = send_json(
            &app,
            Method::POST,
            "/defi/borrow",
            json!({
                "owner": "alice",
                "amount": 100
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], json!(true));
        assert_eq!(body["position"]["debt_amount"], json!(100));

        let (status, body) = send_json(
            &app,
            Method::POST,
            "/defi/withdraw",
            json!({
                "owner": "alice",
                "amount": 10
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["position"]["collateral_amount"], json!(190));

        let (status, body) = send_empty(&app, Method::GET, "/defi/position/alice").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["position"]["owner"], json!("alice"));
        assert_eq!(body["position"]["collateral_amount"], json!(190));

        let (status, body) = send_empty(&app, Method::GET, "/defi/stats").await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["stats"]["position_count"], json!(1));
        assert_eq!(body["stats"]["total_collateral"], json!(190));
        assert_eq!(body["stats"]["total_debt"], json!(100));
    }

    /// 验证健康仓位清算会被拒绝。
    #[tokio::test]
    async fn defi_healthy_position_should_not_liquidate() {
        let app = build_test_app();

        let _ = send_json(
            &app,
            Method::POST,
            "/defi/deposit",
            json!({
                "owner": "alice",
                "amount": 200
            }),
        )
        .await;
        let _ = send_json(
            &app,
            Method::POST,
            "/defi/borrow",
            json!({
                "owner": "alice",
                "amount": 100
            }),
        )
        .await;

        let (status, body) = send_json(
            &app,
            Method::POST,
            "/defi/liquidate",
            json!({
                "borrower": "alice",
                "amount": 20
            }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["ok"], json!(false));
    }

    /// 验证 NFT 铸造、挂单、购买主流程。
    #[tokio::test]
    async fn nft_flow_should_work() {
        let app = build_test_app();

        let (status, body) = send_json(
            &app,
            Method::POST,
            "/nft/mint",
            json!({
                "owner": "alice",
                "name": "Sunset",
                "description": "digital art",
                "image_url": "https://img/1.png"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let token_id = body["token"]["token_id"]
            .as_str()
            .expect("token_id 应存在")
            .to_string();

        let (status, body) = send_json(
            &app,
            Method::POST,
            "/nft/list",
            json!({
                "seller": "alice",
                "token_id": token_id,
                "price": 188
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let listing_id = body["listing"]["listing_id"]
            .as_str()
            .expect("listing_id 应存在")
            .to_string();

        let (status, body) = send_json(
            &app,
            Method::POST,
            "/nft/buy",
            json!({
                "buyer": "bob",
                "listing_id": listing_id
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["outcome"]["token"]["owner"], json!("bob"));
        assert_eq!(body["outcome"]["paid_price"], json!(188));

        let (status, body) = send_empty(&app, Method::GET, "/nft/owner/bob/tokens").await;
        assert_eq!(status, StatusCode::OK);
        let token_count = body["tokens"].as_array().expect("tokens 应为数组").len();
        assert_eq!(token_count, 1);

        let (status, body) = send_empty(&app, Method::GET, "/nft/listings/active").await;
        assert_eq!(status, StatusCode::OK);
        let active_count = body["listings"]
            .as_array()
            .expect("listings 应为数组")
            .len();
        assert_eq!(active_count, 0);
    }

    /// 验证活跃挂单查询会返回未成交挂单。
    #[tokio::test]
    async fn nft_active_listing_should_be_queryable() {
        let app = build_test_app();

        let (status, body) = send_json(
            &app,
            Method::POST,
            "/nft/mint",
            json!({
                "owner": "alice",
                "name": "Forest",
                "description": "digital art",
                "image_url": "https://img/2.png"
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        let token_id = body["token"]["token_id"]
            .as_str()
            .expect("token_id 应存在")
            .to_string();

        let (status, _) = send_json(
            &app,
            Method::POST,
            "/nft/list",
            json!({
                "seller": "alice",
                "token_id": token_id,
                "price": 99
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = send_empty(&app, Method::GET, "/nft/listings/active").await;
        assert_eq!(status, StatusCode::OK);
        let listings = body["listings"].as_array().expect("listings 应为数组");
        assert_eq!(listings.len(), 1);
        assert_eq!(listings[0]["status"], json!("active"));
    }

    /// 验证 NFT 查询不存在资产时返回 404。
    #[tokio::test]
    async fn nft_missing_token_should_return_not_found() {
        let app = build_test_app();

        let (status, body) = send_empty(&app, Method::GET, "/nft/token/not-exist").await;
        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["ok"], json!(false));
    }

    /// 验证查询不存在的历史交易返回 404。
    #[tokio::test]
    async fn history_missing_tx_should_return_not_found() {
        let app = build_test_app();
        let (status, body) = send_empty(&app, Method::GET, "/history/tx/not-exist").await;

        assert_eq!(status, StatusCode::NOT_FOUND);
        assert_eq!(body["ok"], json!(false));
    }

    /// 验证统一交易查询会优先命中待打包交易。
    #[tokio::test]
    async fn chain_tx_query_should_return_pending_first() {
        let app = build_test_app();
        let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
        let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": alice_wallet.address.clone() }),
        )
        .await;

        let mut tx = Transaction::new(
            alice_wallet.address.clone(),
            bob_wallet.address.clone(),
            6,
            Some(b"chain-tx-query-pending".to_vec()),
        );
        tx.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
            .expect("签名应成功");
        let tx_id = tx.id.clone();

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/tx",
            json!({ "transaction": tx }),
        )
        .await;

        let path = format!("/chain/tx/{tx_id}");
        let (status, body) = send_empty(&app, Method::GET, &path).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], json!(true));
        assert_eq!(body["source"], json!("pending"));
        assert_eq!(body["transaction"]["id"], json!(tx_id));
    }

    /// 验证统一交易查询可回退命中历史交易。
    #[tokio::test]
    async fn chain_tx_query_should_fallback_to_history() {
        let app = build_test_app();
        let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
        let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": alice_wallet.address.clone() }),
        )
        .await;

        let mut tx = Transaction::new(
            alice_wallet.address.clone(),
            bob_wallet.address.clone(),
            4,
            Some(b"chain-tx-query-history".to_vec()),
        );
        tx.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
            .expect("签名应成功");
        let tx_id = tx.id.clone();

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/tx",
            json!({ "transaction": tx }),
        )
        .await;
        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": "history-miner" }),
        )
        .await;

        let path = format!("/chain/tx/{tx_id}");
        let (status, body) = send_empty(&app, Method::GET, &path).await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], json!(true));
        assert_eq!(body["source"], json!("history"));
        assert_eq!(body["transaction"]["id"], json!(tx_id));
    }

    /// 验证提交非法合约脚本交易时会被链交易接口拒绝。
    #[tokio::test]
    async fn chain_submit_invalid_contract_call_should_fail() {
        let app = build_test_app();
        let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": alice_wallet.address.clone() }),
        )
        .await;

        let mut tx = Transaction::new_with_kind(
            TransactionKind::ContractCall,
            alice_wallet.address.clone(),
            "contract-demo",
            1,
            0,
            Some(b"WARP 1\n".to_vec()),
        );
        tx.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
            .expect("签名应成功");

        let (status, body) = send_json(
            &app,
            Method::POST,
            "/chain/tx",
            json!({ "transaction": tx }),
        )
        .await;
        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["ok"], json!(false));
    }

    /// 验证合约交易出块后可查询到状态和事件。
    #[tokio::test]
    async fn chain_contract_state_and_events_should_be_queryable() {
        let app = build_test_app();
        let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
        let contract_address = "contract-counter";

        let _ = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": alice_wallet.address.clone() }),
        )
        .await;

        let mut tx = Transaction::new_with_kind(
            TransactionKind::ContractCall,
            alice_wallet.address.clone(),
            contract_address,
            1,
            1,
            Some(b"LOAD_CONST 2\nSTORE counter\nEMIT \"set\"\nHALT\n".to_vec()),
        );
        tx.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
            .expect("签名应成功");

        let (status, _) = send_json(
            &app,
            Method::POST,
            "/chain/tx",
            json!({ "transaction": tx }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, _) = send_json(
            &app,
            Method::POST,
            "/chain/mine",
            json!({ "miner_address": "miner-2" }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);

        let (status, body) = send_empty(
            &app,
            Method::GET,
            &format!("/chain/contract/{contract_address}/state"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["state"]["counter"], json!(2));

        let (status, body) = send_empty(
            &app,
            Method::GET,
            &format!("/chain/contract/{contract_address}/events"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["events"][0], json!("set"));

        let (status, body) = send_empty(
            &app,
            Method::GET,
            &format!("/chain/contract/{contract_address}/field/counter"),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["value_i64"], json!(2));
    }

    /// 验证 VM 编译和执行接口可用。
    #[tokio::test]
    async fn vm_compile_and_execute_should_work() {
        let app = build_test_app();
        let source = r#"
            LOAD_CONST 2
            LOAD_CONST 3
            ADD
            STORE total
            EMIT "sum_ready"
            HALT
        "#;

        let (status, body) = send_json(
            &app,
            Method::POST,
            "/vm/compile",
            json!({ "source": source }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], json!(true));
        assert_eq!(body["instruction_count"], json!(6));

        let (status, body) = send_json(
            &app,
            Method::POST,
            "/vm/execute",
            json!({
                "source": source,
                "max_steps": 32
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
        assert_eq!(body["ok"], json!(true));
        assert_eq!(body["state"]["total"], json!(5));
        assert_eq!(body["events"][0], json!("sum_ready"));
    }

    /// 验证 VM 编译错误会返回 400。
    #[tokio::test]
    async fn vm_compile_invalid_opcode_should_return_bad_request() {
        let app = build_test_app();
        let (status, body) = send_json(
            &app,
            Method::POST,
            "/vm/compile",
            json!({ "source": "WARP 1" }),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["ok"], json!(false));
    }

    /// 验证 VM 运行时错误会返回 400。
    #[tokio::test]
    async fn vm_execute_runtime_error_should_return_bad_request() {
        let app = build_test_app();
        let source = r#"
            LOAD_CONST 7
            LOAD_CONST 0
            DIV
            HALT
        "#;

        let (status, body) = send_json(
            &app,
            Method::POST,
            "/vm/execute",
            json!({
                "source": source
            }),
        )
        .await;

        assert_eq!(status, StatusCode::BAD_REQUEST);
        assert_eq!(body["ok"], json!(false));
    }

    /// 创建测试用路由。
    fn build_test_app() -> Router {
        build_app(default_app_state())
    }

    /// 发送 JSON 请求。
    async fn send_json(
        app: &Router,
        method: Method,
        uri: &str,
        payload: Value,
    ) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .header("content-type", "application/json")
            .body(Body::from(payload.to_string()))
            .expect("请求构造应成功");
        send_request(app, request).await
    }

    /// 发送空载荷请求。
    async fn send_empty(app: &Router, method: Method, uri: &str) -> (StatusCode, Value) {
        let request = Request::builder()
            .method(method)
            .uri(uri)
            .body(Body::empty())
            .expect("请求构造应成功");
        send_request(app, request).await
    }

    /// 发送请求并解析 JSON 响应。
    async fn send_request(app: &Router, request: Request<Body>) -> (StatusCode, Value) {
        let response = app.clone().oneshot(request).await.expect("请求处理应成功");
        let status = response.status();
        let bytes = to_bytes(response.into_body(), usize::MAX)
            .await
            .expect("读取响应体应成功");
        let body = serde_json::from_slice(&bytes).expect("响应应为 JSON");
        (status, body)
    }
}
