use axum::{
    extract::{Path, State},
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
    AppState {
        blockchain: Arc::new(Mutex::new(Blockchain::new(2, 50))),
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
    let state_store = open_state_store(config)?;
    let history_store = open_history_store(config)?;
    Ok(AppState {
        blockchain: Arc::new(Mutex::new(Blockchain::new(
            config.mining_difficulty,
            config.mining_reward,
        ))),
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

/// 构造 API 路由。
fn build_app(shared_state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/wallet/create", post(wallet_create_handler))
        .route("/tx/verify", post(tx_verify_handler))
        .route("/chain/info", get(chain_info_handler))
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
    match with_chain_mut(&state, |chain| {
        chain.add_transaction(payload.transaction.clone())?;
        Ok(json!({
            "ok": true,
            "pending_tx_count": chain.pending_transactions.len()
        }))
    }) {
        Ok(body) => (StatusCode::OK, Json(body)),
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
        Ok((
            block,
            chain.balances(),
            chain.contract_states.clone(),
            chain.contract_events.clone(),
        ))
    }) {
        Ok((block, balances, contract_states, contract_events)) => {
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
                    }
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
