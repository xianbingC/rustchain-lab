use axum::{
    extract::{Path, State},
    http::StatusCode,
    routing::{get, post},
    Json, Router,
};
use rustchain_apps::defi::{DefiError, LendingConfig, LendingPool};
use rustchain_apps::nft::{NftError, NftMarketplace};
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

    let app = build_app(default_app_state());
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
    /// NFT 市场（原型阶段使用内存存储）。
    nft_marketplace: Arc<Mutex<NftMarketplace>>,
}

/// 构造默认应用状态。
fn default_app_state() -> AppState {
    AppState {
        lending_pool: Arc::new(Mutex::new(LendingPool::new(
            LendingConfig::default(),
            now_unix_ts(),
        ))),
        nft_marketplace: Arc::new(Mutex::new(NftMarketplace::new())),
    }
}

/// 构造 API 路由。
fn build_app(shared_state: AppState) -> Router {
    Router::new()
        .route("/health", get(health_handler))
        .route("/wallet/create", post(wallet_create_handler))
        .route("/tx/verify", post(tx_verify_handler))
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
    use serde_json::{json, Value};
    use tower::ServiceExt;

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
