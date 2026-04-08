use axum::{
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use rustchain_common::{logging::init_logging, AppConfig, AppResult};
use rustchain_core::transaction::Transaction;
use rustchain_crypto::wallet::create_wallet;
use serde::Deserialize;
use serde_json::json;
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

    let app = Router::new()
        .route("/health", get(health_handler))
        .route("/wallet/create", post(wallet_create_handler))
        .route("/tx/verify", post(tx_verify_handler));
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
