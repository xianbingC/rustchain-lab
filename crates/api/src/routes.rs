//! 路由注册表。
//!
//! 所有端点集中在此声明，便于一眼看清 API 全貌与路径冲突。

use crate::handlers::{chain, defi, health, nft, p2p, vm, wallet};
use crate::state::AppState;
use axum::{
    routing::{get, post},
    Router,
};

/// 构造 API 路由。
pub(crate) fn build_app(shared_state: AppState) -> Router {
    Router::new()
        .route("/health", get(health::health_handler))
        .route("/health/live", get(health::health_live_handler))
        .route("/health/ready", get(health::health_ready_handler))
        .route("/metrics", get(health::metrics_handler))
        .route("/wallet/create", post(wallet::wallet_create_handler))
        .route(
            "/wallet/import-private",
            post(wallet::wallet_import_private_handler),
        )
        .route("/wallet/restore", post(wallet::wallet_restore_handler))
        .route("/tx/verify", post(wallet::tx_verify_handler))
        .route("/p2p/status", get(p2p::p2p_status_handler))
        .route("/p2p/peers", get(p2p::p2p_peers_handler))
        .route(
            "/p2p/sync-candidates",
            get(p2p::p2p_sync_candidates_handler),
        )
        .route("/p2p/sync-target", get(p2p::p2p_sync_target_handler))
        .route("/p2p/sync-gap", get(p2p::p2p_sync_gap_handler))
        .route("/p2p/sync-plan", get(p2p::p2p_sync_plan_handler))
        .route("/p2p/sync-step", get(p2p::p2p_sync_step_handler))
        .route("/p2p/peers/nearest", get(p2p::p2p_nearest_peers_handler))
        .route("/p2p/dht/buckets", get(p2p::p2p_dht_buckets_handler))
        .route("/p2p/bootstrap", post(p2p::p2p_bootstrap_handler))
        .route("/p2p/discover", post(p2p::p2p_discover_handler))
        .route("/p2p/diagnose", post(p2p::p2p_diagnose_handler))
        .route("/p2p/peer/register", post(p2p::p2p_register_peer_handler))
        .route("/p2p/message", post(p2p::p2p_message_handler))
        .route(
            "/p2p/transport/sessions",
            get(p2p::p2p_transport_sessions_handler),
        )
        .route(
            "/p2p/transport/frame",
            post(p2p::p2p_transport_frame_handler),
        )
        .route("/chain/info", get(chain::chain_info_handler))
        .route("/chain/difficulty", get(chain::chain_difficulty_handler))
        .route("/chain/validate", get(chain::chain_validate_handler))
        .route(
            "/chain/block/latest",
            get(chain::chain_latest_block_handler),
        )
        .route("/chain/blocks", get(chain::chain_blocks_handler))
        .route(
            "/chain/block/:height",
            get(chain::chain_block_by_height_handler),
        )
        .route(
            "/chain/address/:address/txs",
            get(chain::chain_address_txs_handler),
        )
        .route(
            "/chain/address/:address/summary",
            get(chain::chain_address_summary_handler),
        )
        .route(
            "/chain/address/:address/pending-txs",
            get(chain::chain_address_pending_txs_handler),
        )
        .route(
            "/chain/pending-tx/:tx_id",
            get(chain::chain_pending_tx_handler),
        )
        .route("/chain/tx/:tx_id", get(chain::chain_tx_query_handler))
        .route("/chain/mempool", get(chain::chain_mempool_handler))
        .route("/chain/balance/:address", get(chain::chain_balance_handler))
        .route(
            "/chain/contract/:address/state",
            get(chain::chain_contract_state_handler),
        )
        .route(
            "/chain/contract/:address/events",
            get(chain::chain_contract_events_handler),
        )
        .route(
            "/chain/contract/:address/field/:field",
            get(chain::chain_contract_field_handler),
        )
        .route("/chain/tx", post(chain::chain_submit_tx_handler))
        .route("/chain/mine", post(chain::chain_mine_handler))
        .route(
            "/history/block/:block_hash",
            get(chain::history_block_handler),
        )
        .route("/history/tx/:tx_id", get(chain::history_tx_handler))
        .route("/vm/compile", post(vm::vm_compile_handler))
        .route("/vm/execute", post(vm::vm_execute_handler))
        .route("/defi/deposit", post(defi::defi_deposit_handler))
        .route("/defi/borrow", post(defi::defi_borrow_handler))
        .route("/defi/repay", post(defi::defi_repay_handler))
        .route("/defi/withdraw", post(defi::defi_withdraw_handler))
        .route("/defi/liquidate", post(defi::defi_liquidate_handler))
        .route("/defi/position/:owner", get(defi::defi_position_handler))
        .route("/defi/stats", get(defi::defi_stats_handler))
        .route("/nft/mint", post(nft::nft_mint_handler))
        .route("/nft/list", post(nft::nft_list_handler))
        .route("/nft/cancel", post(nft::nft_cancel_handler))
        .route("/nft/buy", post(nft::nft_buy_handler))
        .route("/nft/token/:token_id", get(nft::nft_token_handler))
        .route("/nft/listing/:listing_id", get(nft::nft_listing_handler))
        .route(
            "/nft/listings/active",
            get(nft::nft_active_listings_handler),
        )
        .route(
            "/nft/owner/:owner/tokens",
            get(nft::nft_owner_tokens_handler),
        )
        .with_state(shared_state)
}
