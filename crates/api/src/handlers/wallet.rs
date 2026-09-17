//! 钱包相关接口：创建、私钥导入与备份恢复。

use axum::{http::StatusCode, Json};
use rustchain_core::transaction::Transaction;
use rustchain_crypto::wallet::{
    create_wallet, create_wallet_from_private_key, import_wallet_from_json, Wallet, WalletKeyPair,
};
use serde::Deserialize;
use serde_json::json;

/// 创建钱包请求。
#[derive(Debug, Deserialize)]
pub(crate) struct CreateWalletRequest {
    /// 钱包密码。
    pub(crate) password: String,
}

/// 私钥导入请求。
#[derive(Debug, Deserialize)]
pub(crate) struct ImportPrivateWalletRequest {
    /// 十六进制私钥。
    pub(crate) private_key: String,
    /// 新钱包密码。
    pub(crate) password: String,
}

/// 钱包恢复请求。
#[derive(Debug, Deserialize)]
pub(crate) struct RestoreWalletRequest {
    /// 钱包备份 JSON。
    pub(crate) wallet_json: String,
}

/// 交易验签请求。
#[derive(Debug, Deserialize)]
pub(crate) struct VerifyTxRequest {
    /// 待校验交易。
    pub(crate) transaction: Transaction,
}

/// 把钱包与密钥对统一序列化为接口响应体，避免三处重复拼装。
fn wallet_response(wallet: &Wallet, key_pair: &WalletKeyPair) -> serde_json::Value {
    json!({
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
    })
}

/// 创建钱包接口（原型阶段同时返回密钥对用于学习调试）。
pub(crate) async fn wallet_create_handler(
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
        Ok((wallet, key_pair)) => (StatusCode::OK, Json(wallet_response(&wallet, &key_pair))),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": error.to_string()
            })),
        ),
    }
}

/// 钱包私钥导入接口。
pub(crate) async fn wallet_import_private_handler(
    Json(payload): Json<ImportPrivateWalletRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if payload.private_key.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "private_key 不能为空"
            })),
        );
    }
    if payload.password.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "password 不能为空"
            })),
        );
    }

    match create_wallet_from_private_key(&payload.private_key, &payload.password) {
        Ok((wallet, key_pair)) => (StatusCode::OK, Json(wallet_response(&wallet, &key_pair))),
        Err(error) => (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": error.to_string()
            })),
        ),
    }
}

/// 钱包恢复接口：校验备份 JSON 并返回钱包内容。
pub(crate) async fn wallet_restore_handler(
    Json(payload): Json<RestoreWalletRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    if payload.wallet_json.trim().is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "wallet_json 不能为空"
            })),
        );
    }

    match import_wallet_from_json(&payload.wallet_json) {
        Ok(wallet) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "wallet": {
                    "address": wallet.address,
                    "public_key": wallet.public_key,
                    "encrypted_private_key": wallet.encrypted_private_key,
                    "kdf_salt": wallet.kdf_salt,
                    "private_key_checksum": wallet.private_key_checksum
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
pub(crate) async fn tx_verify_handler(
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
