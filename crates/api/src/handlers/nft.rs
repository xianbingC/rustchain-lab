//! NFT 市场接口：铸造、挂单、取消、购买与查询。

use crate::state::{with_market, with_market_mut, AppState};
use axum::{
    extract::{Path, State},
    http::StatusCode,
    Json,
};
use rustchain_apps::nft::{ListingStatus, NftError};
use serde::Deserialize;
use serde_json::json;

/// NFT 铸造请求。
#[derive(Debug, Deserialize)]
pub(crate) struct NftMintRequest {
    /// 初始持有人地址。
    pub(crate) owner: String,
    /// NFT 名称。
    pub(crate) name: String,
    /// NFT 描述。
    pub(crate) description: String,
    /// 图片链接。
    pub(crate) image_url: String,
}

/// NFT 挂单请求。
#[derive(Debug, Deserialize)]
pub(crate) struct NftListRequest {
    /// 卖家地址。
    pub(crate) seller: String,
    /// NFT 资产 ID。
    pub(crate) token_id: String,
    /// 标价。
    pub(crate) price: u64,
}

/// NFT 取消挂单请求。
#[derive(Debug, Deserialize)]
pub(crate) struct NftCancelRequest {
    /// 卖家地址。
    pub(crate) seller: String,
    /// 挂单 ID。
    pub(crate) listing_id: String,
}

/// NFT 购买请求。
#[derive(Debug, Deserialize)]
pub(crate) struct NftBuyRequest {
    /// 买家地址。
    pub(crate) buyer: String,
    /// 挂单 ID。
    pub(crate) listing_id: String,
}

/// NFT 铸造接口。
pub(crate) async fn nft_mint_handler(
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
pub(crate) async fn nft_list_handler(
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
pub(crate) async fn nft_cancel_handler(
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
pub(crate) async fn nft_buy_handler(
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
pub(crate) async fn nft_token_handler(
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
pub(crate) async fn nft_listing_handler(
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
pub(crate) async fn nft_active_listings_handler(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
    match with_market(&state, |market| {
        let active_listings: Vec<_> = market
            .listings
            .values()
            .filter(|listing| listing.status == ListingStatus::Active)
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
pub(crate) async fn nft_owner_tokens_handler(
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
