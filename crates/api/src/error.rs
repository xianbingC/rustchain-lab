//! 业务错误到 HTTP 响应的统一映射。
//!
//! 所有 handler 通过这里的函数把领域错误转换为 `(StatusCode, Json)`，
//! 保证接口层状态码策略集中在一处，避免各 handler 自行决定。

use axum::http::StatusCode;
use rustchain_apps::nft::NftError;
use rustchain_p2p::P2pError;
use rustchain_storage::error::StorageError;
use rustchain_vm::compiler::CompileError;
use rustchain_vm::runtime::VmError;
use serde_json::json;

/// 统一的接口错误响应类型，便于 handler 直接返回。
///
/// 内部只保存裸 `Value`，由调用方在 `Err` 分支统一包一层 `Json`，
/// 这样 `map_*_error` 既能用于 `with_*` 辅助函数，也能直接构造响应。
pub(crate) type ApiError = (StatusCode, serde_json::Value);

/// NFT 业务错误映射为 HTTP 错误响应。
pub(crate) fn map_nft_error(error: NftError) -> ApiError {
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
///
/// 映射原则：
/// - 结构性/共识性错误意味着本地链数据本身不可信 → 500
/// - 资源不存在（仓位/余额等查询目标缺失）→ 404
/// - 其余（余额不足、签名非法、合约失败等）属于请求问题 → 400
pub(crate) fn map_core_error(error: rustchain_core::error::CoreError) -> ApiError {
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
        rustchain_core::error::CoreError::DefiPositionNotFound { .. } => StatusCode::NOT_FOUND,
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
pub(crate) fn map_storage_error(error: StorageError) -> ApiError {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({
            "ok": false,
            "error": error.to_string()
        }),
    )
}

/// P2P 错误映射为 HTTP 错误响应。
pub(crate) fn map_p2p_error(error: P2pError) -> ApiError {
    let status = match error {
        P2pError::InvalidArgument(_) | P2pError::InvalidMessage(_) => StatusCode::BAD_REQUEST,
        _ => StatusCode::INTERNAL_SERVER_ERROR,
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
pub(crate) fn map_vm_compile_error(error: CompileError) -> ApiError {
    (
        StatusCode::BAD_REQUEST,
        json!({
            "ok": false,
            "error": error.to_string()
        }),
    )
}

/// VM 运行时错误映射为 HTTP 错误响应。
pub(crate) fn map_vm_runtime_error(error: VmError) -> ApiError {
    (
        StatusCode::BAD_REQUEST,
        json!({
            "ok": false,
            "error": error.to_string()
        }),
    )
}

/// 构造锁被污染的 500 响应，避免各处重复拼装。
pub(crate) fn lock_error(lock_name: &str) -> ApiError {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({
            "ok": false,
            "error": format!("{lock_name} 锁异常")
        }),
    )
}

/// 构造 500 内部错误响应。
pub(crate) fn internal_error(message: impl Into<String>) -> ApiError {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        json!({
            "ok": false,
            "error": message.into()
        }),
    )
}
