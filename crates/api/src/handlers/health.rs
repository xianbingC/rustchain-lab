//! 健康探针与 Prometheus 指标接口。

use crate::state::{with_chain, with_history, with_state_store, AppState};
use axum::{
    extract::State,
    http::{header, StatusCode},
    Json,
};
use serde_json::json;

/// 健康检查接口。
pub(crate) async fn health_handler() -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "service": "rustchain-api"
    }))
}

/// 活性探针接口：用于判断 API 进程是否存活。
pub(crate) async fn health_live_handler() -> Json<serde_json::Value> {
    Json(json!({
        "ok": true,
        "probe": "live",
        "service": "rustchain-api"
    }))
}

/// 就绪探针接口：用于判断链状态与存储依赖是否可用。
pub(crate) async fn health_ready_handler(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
    let (chain_height, pending_tx_count) = match with_chain(&state, |chain| {
        let latest = chain.latest_block()?;
        Ok((latest.index, chain.pending_transactions.len()))
    }) {
        Ok(result) => result,
        Err((status, body)) => return (status, Json(body)),
    };

    if let Err((status, body)) = with_state_store(&state, |store| {
        // 读取一个哨兵键，用于验证状态库读路径可用。
        let _ = store.get_balance("__ready_probe__")?;
        Ok(())
    }) {
        return (status, Json(body));
    }

    if let Err((status, body)) = with_history(&state, |history| {
        // 读取一个哨兵键，用于验证历史库读路径可用。
        let _ = history.get_block("__ready_probe__")?;
        Ok(())
    }) {
        return (status, Json(body));
    }

    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "probe": "ready",
            "service": "rustchain-api",
            "chain_height": chain_height,
            "pending_tx_count": pending_tx_count
        })),
    )
}

/// Prometheus 指标接口：输出基础链状态指标。
pub(crate) async fn metrics_handler(
    State(state): State<AppState>,
) -> (StatusCode, [(header::HeaderName, &'static str); 1], String) {
    let (
        chain_id,
        chain_height,
        pending_tx_count,
        peer_count,
        latest_block_difficulty,
        next_block_expected_difficulty,
    ) = match with_chain(&state, |chain| {
        let latest = chain.latest_block()?;
        let next_difficulty = chain.next_block_expected_difficulty()?;
        Ok((
            chain.chain_id.clone(),
            latest.index,
            chain.pending_transactions.len(),
            chain.peers.len(),
            latest.difficulty,
            next_difficulty,
        ))
    }) {
        Ok(metrics) => metrics,
        Err((status, body)) => {
            let error = body["error"].as_str().unwrap_or("metrics unavailable");
            return (
                status,
                [(header::CONTENT_TYPE, "text/plain; charset=utf-8")],
                format!("# metrics_error\nrustchain_metrics_error{{reason=\"{error}\"}} 1\n"),
            );
        }
    };

    let body = format!(
        "# HELP rustchain_up Whether rustchain api process is up.\n\
# TYPE rustchain_up gauge\n\
rustchain_up 1\n\
# HELP rustchain_build_info Build info labeled by service version.\n\
# TYPE rustchain_build_info gauge\n\
rustchain_build_info{{version=\"{}\"}} 1\n\
# HELP rustchain_chain_info Chain identity information.\n\
# TYPE rustchain_chain_info gauge\n\
rustchain_chain_info{{chain_id=\"{}\"}} 1\n\
# HELP rustchain_chain_height Current best block height.\n\
# TYPE rustchain_chain_height gauge\n\
rustchain_chain_height {chain_height}\n\
# HELP rustchain_pending_tx_count Number of pending transactions.\n\
# TYPE rustchain_pending_tx_count gauge\n\
rustchain_pending_tx_count {pending_tx_count}\n\
# HELP rustchain_peer_count Number of connected peers.\n\
# TYPE rustchain_peer_count gauge\n\
rustchain_peer_count {peer_count}\n\
# HELP rustchain_difficulty Expected proof-of-work difficulty for next block.\n\
# TYPE rustchain_difficulty gauge\n\
rustchain_difficulty {next_block_expected_difficulty}\n\
# HELP rustchain_latest_block_difficulty Difficulty recorded on latest block.\n\
# TYPE rustchain_latest_block_difficulty gauge\n\
rustchain_latest_block_difficulty {latest_block_difficulty}\n",
        env!("CARGO_PKG_VERSION"),
        chain_id
    );

    (
        StatusCode::OK,
        [(
            header::CONTENT_TYPE,
            "text/plain; version=0.0.4; charset=utf-8",
        )],
        body,
    )
}
