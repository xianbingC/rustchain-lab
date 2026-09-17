//! P2P 相关接口：节点注册、发现、同步规划、传输帧与入站消息处理。

use crate::chain_ops::{
    accept_synced_block, build_next_get_blocks_request, encode_blocks, encode_transactions,
};
use crate::error::{internal_error, lock_error, map_p2p_error};
use crate::state::{
    with_chain, with_chain_mut, with_p2p, with_p2p_mut, with_transport_sessions, AppState,
};
use axum::{
    extract::{Query, State},
    http::StatusCode,
    Json,
};
use rustchain_core::block::Block;
use rustchain_core::transaction::Transaction;
use rustchain_p2p::{
    engine::OutboundEnvelope, message::NetworkMessage, peer::PeerStatus,
    transport::encode_outbound_frames,
};
use serde::Deserialize;
use serde_json::json;

/// P2P 节点注册请求。
#[derive(Debug, Deserialize)]
pub(crate) struct P2pRegisterPeerRequest {
    /// 节点 ID。
    pub(crate) peer_id: String,
    /// 节点地址。
    pub(crate) address: String,
}

/// P2P 最近邻查询参数。
#[derive(Debug, Deserialize)]
pub(crate) struct P2pNearestPeersQuery {
    /// 查询目标节点 ID。
    pub(crate) target_peer_id: String,
    /// 返回数量上限。
    pub(crate) limit: Option<usize>,
}

/// P2P DHT 桶查询参数。
#[derive(Debug, Deserialize)]
pub(crate) struct P2pDhtBucketsQuery {
    /// 查询目标节点 ID。
    pub(crate) target_peer_id: String,
    /// 桶数量。
    pub(crate) bucket_count: Option<u8>,
}

/// P2P 批量发现请求。
#[derive(Debug, Deserialize)]
pub(crate) struct P2pDiscoverRequest {
    /// 查询目标节点 ID，缺省使用本节点 ID。
    pub(crate) target_peer_id: Option<String>,
    /// 每轮发现返回的节点数量上限。
    pub(crate) limit: Option<u8>,
}

/// P2P 诊断请求。
#[derive(Debug, Deserialize)]
pub(crate) struct P2pDiagnoseRequest {
    /// 诊断目标节点 ID，缺省使用本节点 ID。
    pub(crate) target_peer_id: Option<String>,
    /// 参与诊断的节点数量上限。
    pub(crate) limit: Option<u8>,
}

/// P2P 入站消息模拟请求。
#[derive(Debug, Deserialize)]
pub(crate) struct P2pIncomingMessageRequest {
    /// 来源节点 ID。
    pub(crate) peer_id: String,
    /// 来源节点地址。
    pub(crate) address: String,
    /// 消息序号。
    pub(crate) sequence: u64,
    /// 网络消息内容。
    pub(crate) message: NetworkMessage,
}

/// P2P 传输帧注入请求。
#[derive(Debug, Deserialize)]
pub(crate) struct P2pTransportFrameRequest {
    /// 来源节点 ID。
    pub(crate) peer_id: String,
    /// 来源节点地址。
    pub(crate) address: String,
    /// 原始传输帧字节。
    pub(crate) bytes: Vec<u8>,
}

/// P2P 状态查询接口。
pub(crate) async fn p2p_status_handler(
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
pub(crate) async fn p2p_peers_handler(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
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

/// P2P 同步候选查询接口：返回按优先级排序的候选节点列表。
pub(crate) async fn p2p_sync_candidates_handler(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
    match with_p2p(&state, |engine| Ok(engine.sync_candidates())) {
        Ok(candidates) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "has_candidates": !candidates.is_empty(),
                "candidate_count": candidates.len(),
                "candidates": candidates
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// P2P 同步目标查询接口：返回当前最优拉块节点。
pub(crate) async fn p2p_sync_target_handler(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
    match with_p2p(&state, |engine| Ok(engine.select_sync_target())) {
        Ok(Some(target)) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "has_target": true,
                "target": target
            })),
        ),
        Ok(None) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "has_target": false,
                "target": null
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// P2P 同步差距查询接口：展示当前落后高度与追平批次数估算。
pub(crate) async fn p2p_sync_gap_handler(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
    let local_height = match with_chain(&state, |chain| Ok(chain.latest_block()?.index)) {
        Ok(height) => height,
        Err((status, body)) => return (status, Json(body)),
    };
    let target = match with_p2p(&state, |engine| Ok(engine.select_sync_target())) {
        Ok(target) => target,
        Err((status, body)) => return (status, Json(body)),
    };

    let Some(target) = target else {
        return (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "has_target": false,
                "local_height": local_height,
                "target_height": local_height,
                "gap": 0,
                "estimated_batches": 0,
                "next_request": null
            })),
        );
    };

    let target_height = target.best_height;
    let gap = target_height.saturating_sub(local_height);
    let estimated_batches = if gap == 0 { 0 } else { (gap + 127) / 128 };
    let next_request = match build_next_get_blocks_request(&state, &target.id) {
        Ok(request) => request,
        Err((status, body)) => return (status, Json(body)),
    };

    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "has_target": true,
            "target": target,
            "local_height": local_height,
            "target_height": target_height,
            "gap": gap,
            "estimated_batches": estimated_batches,
            "next_request": next_request
        })),
    )
}

/// P2P 同步计划接口：返回当前建议发送的拉块请求。
pub(crate) async fn p2p_sync_plan_handler(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
    let target = match with_p2p(&state, |engine| Ok(engine.select_sync_target())) {
        Ok(target) => target,
        Err((status, body)) => return (status, Json(body)),
    };

    let Some(target) = target else {
        return (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "has_target": false,
                "has_plan": false,
                "target": null,
                "plan": null
            })),
        );
    };

    match build_next_get_blocks_request(&state, &target.id) {
        Ok(Some(plan)) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "has_target": true,
                "has_plan": true,
                "target": target,
                "plan": plan
            })),
        ),
        Ok(None) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "has_target": true,
                "has_plan": false,
                "target": target,
                "plan": null
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// P2P 单步同步接口：返回下一条可直接执行的同步动作。
pub(crate) async fn p2p_sync_step_handler(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
    let target = match with_p2p(&state, |engine| Ok(engine.select_sync_target())) {
        Ok(target) => target,
        Err((status, body)) => return (status, Json(body)),
    };

    let Some(target) = target else {
        return (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "has_action": false,
                "reason": "没有可用同步目标节点",
                "action": null
            })),
        );
    };

    match build_next_get_blocks_request(&state, &target.id) {
        Ok(Some(message)) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "has_action": true,
                "reason": "建议向最优同步目标发送拉块请求",
                "target": target,
                "action": {
                    "target_peer_id": target.id,
                    "message": message
                }
            })),
        ),
        Ok(None) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "has_action": false,
                "reason": "本地高度已追平或无需继续拉块",
                "target": target,
                "action": null
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// P2P 最近邻节点查询接口。
pub(crate) async fn p2p_nearest_peers_handler(
    State(state): State<AppState>,
    Query(query): Query<P2pNearestPeersQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    let target_peer_id = query.target_peer_id.trim();
    if target_peer_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "target_peer_id 不能为空"
            })),
        );
    }

    let limit = query.limit.unwrap_or(8);
    if limit == 0 || limit > 64 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "limit 必须在 1~64 之间"
            })),
        );
    }

    match with_p2p(&state, |engine| engine.nearest_peers(target_peer_id, limit)) {
        Ok(nearest) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "target_peer_id": target_peer_id,
                "limit": limit,
                "count": nearest.len(),
                "peers": nearest
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// P2P DHT 桶视图查询接口。
pub(crate) async fn p2p_dht_buckets_handler(
    State(state): State<AppState>,
    Query(query): Query<P2pDhtBucketsQuery>,
) -> (StatusCode, Json<serde_json::Value>) {
    let target_peer_id = query.target_peer_id.trim();
    if target_peer_id.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "target_peer_id 不能为空"
            })),
        );
    }

    let bucket_count = query.bucket_count.unwrap_or(8);
    if bucket_count == 0 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "bucket_count 必须大于 0"
            })),
        );
    }

    match with_p2p(&state, |engine| {
        engine.dht_buckets(target_peer_id, bucket_count)
    }) {
        Ok(buckets) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "target_peer_id": target_peer_id,
                "bucket_count": bucket_count,
                "buckets": buckets
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// P2P 启动握手接口：为未连接节点生成握手消息。
pub(crate) async fn p2p_bootstrap_handler(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
    let listen_addr = state.p2p_listen_addr.clone();
    let protocol_version = state.p2p_protocol_version.clone();
    match with_p2p(&state, |engine| {
        engine.build_bootstrap_handshakes(&listen_addr, &protocol_version)
    }) {
        Ok(outbound) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "listen_addr": listen_addr,
                "protocol_version": protocol_version,
                "outbound_count": outbound.len(),
                "outbound": outbound
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// P2P 批量发现接口：为已知节点生成一轮 FindNode 请求。
pub(crate) async fn p2p_discover_handler(
    State(state): State<AppState>,
    Json(payload): Json<P2pDiscoverRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let limit = payload.limit.unwrap_or(8);
    if limit == 0 || limit > 64 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "limit 必须在 1~64 之间"
            })),
        );
    }

    let target_peer_id = payload
        .target_peer_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    match with_p2p(&state, |engine| {
        let target = target_peer_id
            .clone()
            .unwrap_or_else(|| engine.local_peer_id().to_string());
        let outbound = engine.build_find_node_requests(&target, limit)?;
        Ok((target, outbound))
    }) {
        Ok((target, outbound)) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "target_peer_id": target,
                "limit": limit,
                "outbound_count": outbound.len(),
                "outbound": outbound
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// P2P 诊断接口：汇总节点发现状态并给出下一步动作建议。
pub(crate) async fn p2p_diagnose_handler(
    State(state): State<AppState>,
    Json(payload): Json<P2pDiagnoseRequest>,
) -> (StatusCode, Json<serde_json::Value>) {
    let limit = payload.limit.unwrap_or(8);
    if limit == 0 || limit > 64 {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "limit 必须在 1~64 之间"
            })),
        );
    }

    let listen_addr = state.p2p_listen_addr.clone();
    let protocol_version = state.p2p_protocol_version.clone();
    let target_peer_id = payload
        .target_peer_id
        .as_deref()
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(ToOwned::to_owned);

    match with_p2p(&state, |engine| {
        let target = target_peer_id
            .clone()
            .unwrap_or_else(|| engine.local_peer_id().to_string());
        let snapshot = engine.peer_snapshot();
        let connected_count = snapshot
            .iter()
            .filter(|peer| peer.status == PeerStatus::Connected)
            .count();
        let bootstrap_outbound =
            engine.build_bootstrap_handshakes(&listen_addr, &protocol_version)?;
        let discover_outbound = engine.build_find_node_requests(&target, limit)?;
        let nearest_peers = engine.nearest_peers(&target, limit as usize)?;

        let mut suggestions = Vec::new();
        if snapshot.is_empty() {
            suggestions.push("当前没有已知节点，请先 register-peer 或配置 seed_nodes".to_string());
        }
        if !bootstrap_outbound.is_empty() {
            suggestions.push(format!(
                "建议先发送 bootstrap 握手，共 {} 条待发送消息",
                bootstrap_outbound.len()
            ));
        }
        if !discover_outbound.is_empty() {
            suggestions.push(format!(
                "建议随后发送 discover 请求，共 {} 条待发送消息",
                discover_outbound.len()
            ));
        }
        if nearest_peers.is_empty() {
            suggestions.push("暂无最近邻结果，等待 nodes 响应后再继续诊断".to_string());
        }
        if connected_count == 0 && !snapshot.is_empty() {
            suggestions.push("当前已知节点均未连接，可先执行 bootstrap 建立握手".to_string());
        }

        Ok((
            target,
            snapshot.len(),
            connected_count,
            bootstrap_outbound,
            discover_outbound,
            nearest_peers,
            suggestions,
        ))
    }) {
        Ok((
            target,
            peer_count,
            connected_count,
            bootstrap_outbound,
            discover_outbound,
            nearest_peers,
            suggestions,
        )) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "target_peer_id": target,
                "limit": limit,
                "peer_count": peer_count,
                "connected_count": connected_count,
                "bootstrap_outbound_count": bootstrap_outbound.len(),
                "discover_outbound_count": discover_outbound.len(),
                "nearest_peer_count": nearest_peers.len(),
                "bootstrap_outbound": bootstrap_outbound,
                "discover_outbound": discover_outbound,
                "nearest_peers": nearest_peers,
                "suggestions": suggestions
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// P2P 节点注册接口。
pub(crate) async fn p2p_register_peer_handler(
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

/// P2P 传输会话快照接口。
pub(crate) async fn p2p_transport_sessions_handler(
    State(state): State<AppState>,
) -> (StatusCode, Json<serde_json::Value>) {
    match with_transport_sessions(&state, |sessions| Ok(sessions.snapshot())) {
        Ok(sessions) => (
            StatusCode::OK,
            Json(json!({
                "ok": true,
                "session_count": sessions.len(),
                "sessions": sessions
            })),
        ),
        Err((status, body)) => (status, Json(body)),
    }
}

/// P2P 入站传输帧接口，用于模拟真实 TCP/Noise 解密后的字节流。
///
/// 注意：这里需要同时持有 engine 与 sessions 两把锁（会话池要驱动引擎），
/// 因此不能复用 `with_*` 辅助函数，必须直接访问状态字段。
pub(crate) async fn p2p_transport_frame_handler(
    State(state): State<AppState>,
    Json(payload): Json<P2pTransportFrameRequest>,
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
    if payload.bytes.is_empty() {
        return (
            StatusCode::BAD_REQUEST,
            Json(json!({
                "ok": false,
                "error": "bytes 不能为空"
            })),
        );
    }

    let peer_id = payload.peer_id.trim().to_string();
    let address = payload.address.trim().to_string();
    let bytes_received = payload.bytes.len();

    let mut engine = match state.p2p_engine.lock() {
        Ok(engine) => engine,
        Err(_) => {
            let (status, body) = lock_error("p2p_engine");
            return (status, Json(body));
        }
    };
    let mut sessions = match state.p2p_transport_sessions.lock() {
        Ok(sessions) => sessions,
        Err(_) => {
            let (status, body) = lock_error("p2p_transport_sessions");
            return (status, Json(body));
        }
    };

    let report = match sessions.push_inbound_bytes(&mut engine, peer_id, address, &payload.bytes) {
        Ok(report) => report,
        Err(error) => {
            let (status, body) = map_p2p_error(error);
            return (status, Json(body));
        }
    };
    let outbound_frames = match encode_outbound_frames(&report.outbound) {
        Ok(frames) => frames,
        Err(error) => {
            let (status, body) = map_p2p_error(error);
            return (status, Json(body));
        }
    };
    let session_snapshot = sessions.snapshot();

    (
        StatusCode::OK,
        Json(json!({
            "ok": true,
            "bytes_received": bytes_received,
            "processed": report.processed,
            "outbound_count": report.outbound.len(),
            "outbound": report.outbound,
            "outbound_frame_count": outbound_frames.len(),
            "outbound_frames": outbound_frames,
            "session_count": session_snapshot.len(),
            "sessions": session_snapshot
        })),
    )
}

/// P2P 入站消息模拟接口。
pub(crate) async fn p2p_message_handler(
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
                    let (status, body) = internal_error(format!("反序列化交易失败: {error}"));
                    return (status, Json(body));
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
                    let (status, body) = internal_error(format!("反序列化区块失败: {error}"));
                    return (status, Json(body));
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
                        let (status, body) = internal_error(format!("反序列化区块失败: {error}"));
                        return (status, Json(body));
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
                        let (status, body) = internal_error(format!("反序列化交易失败: {error}"));
                        return (status, Json(body));
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
                    let (status, body) = internal_error(format!("交易编码失败: {error}"));
                    return (status, Json(body));
                }
            };
            replace_or_push_mempool(&mut report.outbound, &peer_id, encoded_transactions);
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
                    let (status, body) = internal_error(format!("区块编码失败: {error}"));
                    return (status, Json(body));
                }
            };
            replace_or_push_blocks(&mut report.outbound, &peer_id, encoded_blocks);
        }
        NetworkMessage::Nodes { peers } => {
            // 将发现节点同步到链节点列表，便于链侧统计和运维观察。
            if let Err((status, body)) = with_chain_mut(&state, |chain| {
                for peer in peers {
                    chain.add_peer(peer.address);
                }
                Ok(())
            }) {
                return (status, Json(body));
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

/// 用真实交易池数据替换引擎预置的空 Mempool 响应，找不到再追加。
///
/// 引擎在收到 GetMempool 时会先产生一个空 Mempool 出站消息，这里做原地替换
/// 以避免同一次响应里出现两条重复消息。
fn replace_or_push_mempool(
    outbound: &mut Vec<OutboundEnvelope>,
    peer_id: &str,
    transactions: Vec<Vec<u8>>,
) {
    for envelope in outbound.iter_mut() {
        if envelope.target_peer_id == peer_id
            && matches!(envelope.message, NetworkMessage::Mempool { .. })
        {
            envelope.message = NetworkMessage::Mempool {
                transactions: transactions.clone(),
            };
            return;
        }
    }

    outbound.push(OutboundEnvelope {
        target_peer_id: peer_id.to_string(),
        message: NetworkMessage::Mempool { transactions },
    });
}

/// 用真实区块数据替换引擎预置的空 Blocks 响应，找不到再追加。
fn replace_or_push_blocks(
    outbound: &mut Vec<OutboundEnvelope>,
    peer_id: &str,
    blocks: Vec<Vec<u8>>,
) {
    for envelope in outbound.iter_mut() {
        if envelope.target_peer_id == peer_id
            && matches!(envelope.message, NetworkMessage::Blocks { .. })
        {
            envelope.message = NetworkMessage::Blocks {
                blocks: blocks.clone(),
            };
            return;
        }
    }

    outbound.push(OutboundEnvelope {
        target_peer_id: peer_id.to_string(),
        message: NetworkMessage::Blocks { blocks },
    });
}
