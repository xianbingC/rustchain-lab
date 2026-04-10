use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use rustchain_apps::defi::{DefiError, LendingConfig, LendingPool};
use rustchain_common::{logging::init_logging, AppConfig, AppResult};
use rustchain_core::transaction::Transaction;
use rustchain_crypto::wallet::create_wallet;
use serde::{Deserialize, Serialize};
use serde_json::json;
use std::{
    sync::{Arc, Mutex},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::net::TcpListener;

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

    let shared_state = AppState {
        lending_pool: Arc::new(Mutex::new(LendingPool::new(
            LendingConfig::default(),
            now_unix_ts(),
        ))),
    };

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/wallet/create", post(wallet_create_handler))
        .route("/tx/verify", post(tx_verify_handler))
        .route("/defi/deposit", post(defi_deposit_handler))
        .route("/defi/borrow", post(defi_borrow_handler))
        .route("/defi/repay", post(defi_repay_handler))
        .route("/defi/position/:owner", get(defi_position_handler))
        .with_state(shared_state);
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
    /// DeFi 借贷池（原型阶段使用内存存储）。
    lending_pool: Arc<Mutex<LendingPool>>,
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

/// DeFi 请求载荷。
#[derive(Debug, Deserialize)]
struct DefiActionRequest {
    /// 用户地址。
    owner: String,
    /// 操作数量。
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

/// 获取当前 Unix 秒级时间戳。
fn now_unix_ts() -> i64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs() as i64)
        .unwrap_or(0)
}
