//! API 集成测试。
//!
//! 通过 `tower::ServiceExt::oneshot` 直接驱动 Router，覆盖所有 HTTP 端点的
//! 主流程与关键失败分支。

use crate::routes::build_app;
use crate::state::{
    bootstrap_seed_nodes, chain_status_from_blockchain, default_app_state, parse_seed_node_entry,
};
use axum::{
    body::{to_bytes, Body},
    http::{Method, Request, StatusCode},
    Router,
};
use rustchain_core::block::Block;
use rustchain_core::blockchain::Blockchain;
use rustchain_core::transaction::{Transaction, TransactionKind};
use rustchain_crypto::wallet::create_wallet;
use rustchain_p2p::{codec::FramedMessageCodec, message::NetworkMessage};
use serde_json::{json, Value};
use tower::ServiceExt;

/// 验证种子节点配置支持 peer_id@address 格式。
#[test]
fn parse_seed_node_entry_should_support_peer_and_address() {
    let parsed = parse_seed_node_entry("seed-a@/ip4/127.0.0.1/tcp/7001", 0).expect("应解析成功");
    assert_eq!(parsed.0, "seed-a");
    assert_eq!(parsed.1, "/ip4/127.0.0.1/tcp/7001");
}

/// 验证仅地址格式会自动生成节点 ID。
#[test]
fn parse_seed_node_entry_should_generate_peer_id_for_address_only() {
    let parsed = parse_seed_node_entry("/ip4/127.0.0.1/tcp/7002", 1).expect("应解析成功");
    assert_eq!(parsed.0, "seed-2");
    assert_eq!(parsed.1, "/ip4/127.0.0.1/tcp/7002");
}

/// 验证种子节点导入会同步更新 P2P 引擎和链节点列表。
#[test]
fn bootstrap_seed_nodes_should_register_to_engine_and_chain() {
    let mut chain = Blockchain::new(2, 50);
    let chain_status = chain_status_from_blockchain(&chain);
    let mut engine = rustchain_p2p::engine::SyncEngine::new("local-node", chain_status);
    let seed_nodes = vec![
        "seed-a@/ip4/127.0.0.1/tcp/7001".to_string(),
        "/ip4/127.0.0.1/tcp/7002".to_string(),
    ];

    let imported = bootstrap_seed_nodes(&mut engine, &mut chain, &seed_nodes);
    assert_eq!(imported, 2);
    assert_eq!(engine.peer_count(), 2);
    assert_eq!(chain.peers.len(), 2);
    assert!(engine.peers().get("seed-a").is_some());
    assert!(engine.peers().get("seed-2").is_some());
}

/// 验证基础健康检查接口可用。
#[tokio::test]
async fn health_should_return_ok() {
    let app = build_test_app();
    let (status, body) = send_empty(&app, Method::GET, "/health").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["status"], json!("ok"));
    assert_eq!(body["service"], json!("rustchain-api"));
}

/// 验证活性探针接口可用。
#[tokio::test]
async fn health_live_should_return_ok() {
    let app = build_test_app();
    let (status, body) = send_empty(&app, Method::GET, "/health/live").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["probe"], json!("live"));
}

/// 验证就绪探针接口可用。
#[tokio::test]
async fn health_ready_should_return_ok() {
    let app = build_test_app();
    let (status, body) = send_empty(&app, Method::GET, "/health/ready").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["probe"], json!("ready"));
    assert_eq!(body["chain_height"], json!(0));
    assert_eq!(body["pending_tx_count"], json!(0));
}

/// 验证钱包私钥导入和恢复接口可用。
#[tokio::test]
async fn wallet_import_private_and_restore_should_work() {
    let app = build_test_app();
    let (_, key_pair) = create_wallet("source-pass").expect("创建钱包应成功");

    let (status, body) = send_json(
        &app,
        Method::POST,
        "/wallet/import-private",
        json!({
            "private_key": key_pair.private_key,
            "password": "import-pass"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["wallet"]["address"], json!(key_pair.address));

    let wallet_json = serde_json::to_string(&body["wallet"]).expect("序列化钱包应成功");
    let (status, body) = send_json(
        &app,
        Method::POST,
        "/wallet/restore",
        json!({
            "wallet_json": wallet_json
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["wallet"]["address"], json!(key_pair.address));
}

/// 验证空钱包恢复请求会被拒绝。
#[tokio::test]
async fn wallet_restore_with_empty_payload_should_fail() {
    let app = build_test_app();
    let (status, body) = send_json(
        &app,
        Method::POST,
        "/wallet/restore",
        json!({
            "wallet_json": ""
        }),
    )
    .await;

    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["ok"], json!(false));
}

/// 验证 Prometheus 指标接口可用。
#[tokio::test]
async fn metrics_should_return_prometheus_format() {
    let app = build_test_app();
    let (status, body) = send_text(&app, Method::GET, "/metrics").await;
    assert_eq!(status, StatusCode::OK);
    assert!(body.contains("rustchain_up 1"));
    assert!(body.contains("rustchain_build_info"));
    assert!(body.contains("rustchain_chain_info{chain_id=\"rustchain-lab-dev\"} 1"));
    assert!(body.contains("rustchain_chain_height 0"));
    assert!(body.contains("rustchain_pending_tx_count 0"));
    assert!(body.contains("rustchain_peer_count 0"));
    assert!(body.contains("rustchain_difficulty 2"));
    assert!(body.contains("rustchain_latest_block_difficulty 0"));
}

/// 验证链信息、交易提交、挖矿和余额查询主流程。
#[tokio::test]
async fn chain_flow_should_work() {
    let app = build_test_app();
    let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
    let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

    let (status, body) = send_empty(&app, Method::GET, "/chain/info").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["chain"]["height"], json!(0));
    assert_eq!(body["chain"]["difficulty"], json!(2));
    assert_eq!(body["chain"]["next_block_expected_difficulty"], json!(2));
    assert_eq!(body["chain"]["latest_block_difficulty"], json!(0));
    assert_eq!(body["chain"]["target_block_time_secs"], json!(10));
    assert_eq!(body["chain"]["difficulty_adjustment_interval"], json!(10));

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

/// 验证链难度详情接口返回最新难度和下一块难度。
#[tokio::test]
async fn chain_difficulty_endpoint_should_work() {
    let app = build_test_app();

    let (status, body) = send_empty(&app, Method::GET, "/chain/difficulty").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["difficulty"]["height"], json!(0));
    assert_eq!(body["difficulty"]["latest_block_difficulty"], json!(0));
    assert_eq!(
        body["difficulty"]["next_block_expected_difficulty"],
        json!(2)
    );
    assert_eq!(body["difficulty"]["target_block_time_secs"], json!(10));
    assert_eq!(
        body["difficulty"]["difficulty_adjustment_interval"],
        json!(10)
    );
}

/// 验证交易池查询支持返回完整列表和 limit 截断。
#[tokio::test]
async fn chain_mempool_should_support_limit_query() {
    let app = build_test_app();
    let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
    let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": alice_wallet.address.clone() }),
    )
    .await;

    let mut tx1 = Transaction::new(
        alice_wallet.address.clone(),
        bob_wallet.address.clone(),
        10,
        Some(b"mempool-test-1".to_vec()),
    );
    tx1.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
        .expect("签名应成功");
    let tx1_id = tx1.id.clone();

    let mut tx2 = Transaction::new(
        alice_wallet.address.clone(),
        bob_wallet.address.clone(),
        5,
        Some(b"mempool-test-2".to_vec()),
    );
    tx2.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
        .expect("签名应成功");
    let tx2_id = tx2.id.clone();

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/tx",
        json!({ "transaction": tx1 }),
    )
    .await;
    let _ = send_json(
        &app,
        Method::POST,
        "/chain/tx",
        json!({ "transaction": tx2 }),
    )
    .await;

    let (status, body) = send_empty(&app, Method::GET, "/chain/mempool").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total_pending_tx_count"], json!(2));
    assert_eq!(body["returned_count"], json!(2));

    let all_txs = body["transactions"]
        .as_array()
        .expect("transactions 应为数组");
    assert_eq!(all_txs.len(), 2);
    let all_ids = all_txs
        .iter()
        .filter_map(|tx| tx["id"].as_str())
        .collect::<Vec<_>>();
    assert!(all_ids.contains(&tx1_id.as_str()));
    assert!(all_ids.contains(&tx2_id.as_str()));

    let (status, body) = send_empty(&app, Method::GET, "/chain/mempool?limit=1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total_pending_tx_count"], json!(2));
    assert_eq!(body["returned_count"], json!(1));
    assert_eq!(
        body["transactions"]
            .as_array()
            .expect("transactions 应为数组")
            .len(),
        1
    );
}

/// 验证交易池查询支持 offset 分页。
#[tokio::test]
async fn chain_mempool_should_support_offset_query() {
    let app = build_test_app();
    let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
    let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": alice_wallet.address.clone() }),
    )
    .await;

    let mut tx1 = Transaction::new(
        alice_wallet.address.clone(),
        bob_wallet.address.clone(),
        10,
        Some(b"mempool-offset-1".to_vec()),
    );
    tx1.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
        .expect("签名应成功");
    let tx1_id = tx1.id.clone();

    let mut tx2 = Transaction::new(
        alice_wallet.address.clone(),
        bob_wallet.address.clone(),
        6,
        Some(b"mempool-offset-2".to_vec()),
    );
    tx2.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
        .expect("签名应成功");
    let tx2_id = tx2.id.clone();

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/tx",
        json!({ "transaction": tx1 }),
    )
    .await;
    let _ = send_json(
        &app,
        Method::POST,
        "/chain/tx",
        json!({ "transaction": tx2 }),
    )
    .await;

    let (status, body) = send_empty(&app, Method::GET, "/chain/mempool?offset=1&limit=1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["offset"], json!(1));
    assert_eq!(body["total_pending_tx_count"], json!(2));
    assert_eq!(body["matched_count"], json!(2));
    assert_eq!(body["returned_count"], json!(1));
    assert_eq!(body["transactions"][0]["id"], json!(tx2_id));
    assert_ne!(body["transactions"][0]["id"], json!(tx1_id));
}

/// 验证交易池查询 limit=0 会被拒绝。
#[tokio::test]
async fn chain_mempool_with_zero_limit_should_fail() {
    let app = build_test_app();
    let (status, body) = send_empty(&app, Method::GET, "/chain/mempool?limit=0").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["ok"], json!(false));
}

/// 验证交易池查询支持按地址过滤（from/to 任一匹配）。
#[tokio::test]
async fn chain_mempool_should_support_address_filter() {
    let app = build_test_app();
    let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
    let (carol_wallet, carol_key_pair) = create_wallet("carol-pass").expect("创建钱包应成功");
    let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": alice_wallet.address.clone() }),
    )
    .await;
    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": carol_wallet.address.clone() }),
    )
    .await;

    let mut tx1 = Transaction::new(
        alice_wallet.address.clone(),
        bob_wallet.address.clone(),
        8,
        Some(b"mempool-filter-alice".to_vec()),
    );
    tx1.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
        .expect("签名应成功");
    let tx1_id = tx1.id.clone();

    let mut tx2 = Transaction::new(
        carol_wallet.address.clone(),
        bob_wallet.address.clone(),
        9,
        Some(b"mempool-filter-carol".to_vec()),
    );
    tx2.sign_with_private_key(&carol_key_pair.private_key, &carol_key_pair.public_key)
        .expect("签名应成功");

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/tx",
        json!({ "transaction": tx1 }),
    )
    .await;
    let _ = send_json(
        &app,
        Method::POST,
        "/chain/tx",
        json!({ "transaction": tx2 }),
    )
    .await;

    let path = format!("/chain/mempool?address={}", alice_wallet.address);
    let (status, body) = send_empty(&app, Method::GET, &path).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["total_pending_tx_count"], json!(2));
    assert_eq!(body["matched_count"], json!(1));
    assert_eq!(body["returned_count"], json!(1));
    assert_eq!(body["transactions"][0]["id"], json!(tx1_id));
}

/// 验证交易池地址过滤参数不能为空字符串。
#[tokio::test]
async fn chain_mempool_with_empty_address_should_fail() {
    let app = build_test_app();
    let (status, body) = send_empty(&app, Method::GET, "/chain/mempool?address=").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["ok"], json!(false));
}

/// 验证可按 tx_id 查询待打包交易详情。
#[tokio::test]
async fn chain_pending_tx_should_return_transaction() {
    let app = build_test_app();
    let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
    let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": alice_wallet.address.clone() }),
    )
    .await;

    let mut tx = Transaction::new(
        alice_wallet.address.clone(),
        bob_wallet.address.clone(),
        11,
        Some(b"pending-tx-query".to_vec()),
    );
    tx.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
        .expect("签名应成功");
    let tx_id = tx.id.clone();

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/tx",
        json!({ "transaction": tx }),
    )
    .await;

    let path = format!("/chain/pending-tx/{tx_id}");
    let (status, body) = send_empty(&app, Method::GET, &path).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["transaction"]["id"], json!(tx_id));
}

/// 验证查询不存在的待打包交易返回 404。
#[tokio::test]
async fn chain_pending_tx_not_found_should_return_404() {
    let app = build_test_app();
    let (status, body) = send_empty(&app, Method::GET, "/chain/pending-tx/tx-missing").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["ok"], json!(false));
}

/// 验证可按地址查询已确认交易列表。
#[tokio::test]
async fn chain_address_txs_should_return_confirmed_transactions() {
    let app = build_test_app();
    let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
    let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": alice_wallet.address.clone() }),
    )
    .await;

    let mut tx = Transaction::new(
        alice_wallet.address.clone(),
        bob_wallet.address.clone(),
        12,
        Some(b"address-txs-query".to_vec()),
    );
    tx.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
        .expect("签名应成功");
    let tx_id = tx.id.clone();

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/tx",
        json!({ "transaction": tx }),
    )
    .await;
    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": "collector-miner" }),
    )
    .await;

    let path = format!("/chain/address/{}/txs", bob_wallet.address);
    let (status, body) = send_empty(&app, Method::GET, &path).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["returned_count"], json!(1));
    assert_eq!(body["transactions"][0]["transaction"]["id"], json!(tx_id));
}

/// 验证地址交易查询 limit=0 会被拒绝。
#[tokio::test]
async fn chain_address_txs_with_zero_limit_should_fail() {
    let app = build_test_app();
    let (status, body) = send_empty(&app, Method::GET, "/chain/address/alice/txs?limit=0").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["ok"], json!(false));
}

/// 验证地址交易查询支持方向过滤（in/out）。
#[tokio::test]
async fn chain_address_txs_should_support_direction_filter() {
    let app = build_test_app();
    let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
    let (bob_wallet, bob_key_pair) = create_wallet("bob-pass").expect("创建钱包应成功");
    let (carol_wallet, _) = create_wallet("carol-pass").expect("创建钱包应成功");

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": alice_wallet.address.clone() }),
    )
    .await;

    let mut tx_in = Transaction::new(
        alice_wallet.address.clone(),
        bob_wallet.address.clone(),
        15,
        Some(b"address-txs-direction-in".to_vec()),
    );
    tx_in
        .sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
        .expect("签名应成功");
    let tx_in_id = tx_in.id.clone();

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/tx",
        json!({ "transaction": tx_in }),
    )
    .await;
    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": "direction-miner-a" }),
    )
    .await;

    let mut tx_out = Transaction::new(
        bob_wallet.address.clone(),
        carol_wallet.address.clone(),
        6,
        Some(b"address-txs-direction-out".to_vec()),
    );
    tx_out
        .sign_with_private_key(&bob_key_pair.private_key, &bob_key_pair.public_key)
        .expect("签名应成功");
    let tx_out_id = tx_out.id.clone();

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/tx",
        json!({ "transaction": tx_out }),
    )
    .await;
    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": "direction-miner-b" }),
    )
    .await;

    let path_in = format!("/chain/address/{}/txs?direction=in", bob_wallet.address);
    let (status, body) = send_empty(&app, Method::GET, &path_in).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["returned_count"], json!(1));
    assert_eq!(
        body["transactions"][0]["transaction"]["id"],
        json!(tx_in_id)
    );

    let path_out = format!("/chain/address/{}/txs?direction=out", bob_wallet.address);
    let (status, body) = send_empty(&app, Method::GET, &path_out).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["returned_count"], json!(1));
    assert_eq!(
        body["transactions"][0]["transaction"]["id"],
        json!(tx_out_id)
    );
}

/// 验证地址已确认交易查询支持 offset 分页。
#[tokio::test]
async fn chain_address_txs_should_support_offset_pagination() {
    let app = build_test_app();
    let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
    let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": alice_wallet.address.clone() }),
    )
    .await;

    let mut tx_old = Transaction::new(
        alice_wallet.address.clone(),
        bob_wallet.address.clone(),
        5,
        Some(b"address-txs-offset-old".to_vec()),
    );
    tx_old
        .sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
        .expect("签名应成功");
    let tx_old_id = tx_old.id.clone();
    let _ = send_json(
        &app,
        Method::POST,
        "/chain/tx",
        json!({ "transaction": tx_old }),
    )
    .await;
    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": "offset-miner-a" }),
    )
    .await;

    let mut tx_new = Transaction::new(
        alice_wallet.address.clone(),
        bob_wallet.address.clone(),
        6,
        Some(b"address-txs-offset-new".to_vec()),
    );
    tx_new
        .sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
        .expect("签名应成功");
    let _ = send_json(
        &app,
        Method::POST,
        "/chain/tx",
        json!({ "transaction": tx_new }),
    )
    .await;
    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": "offset-miner-b" }),
    )
    .await;

    let path = format!(
        "/chain/address/{}/txs?direction=in&limit=1&offset=1",
        bob_wallet.address
    );
    let (status, body) = send_empty(&app, Method::GET, &path).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["offset"], json!(1));
    assert_eq!(body["total_count"], json!(2));
    assert_eq!(body["returned_count"], json!(1));
    assert_eq!(
        body["transactions"][0]["transaction"]["id"],
        json!(tx_old_id)
    );
}

/// 验证地址交易查询传入非法 direction 会被拒绝。
#[tokio::test]
async fn chain_address_txs_with_invalid_direction_should_fail() {
    let app = build_test_app();
    let (status, body) = send_empty(
        &app,
        Method::GET,
        "/chain/address/alice/txs?direction=sideways",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["ok"], json!(false));
}

/// 验证可按地址查询待打包交易列表。
#[tokio::test]
async fn chain_address_pending_txs_should_return_transactions() {
    let app = build_test_app();
    let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
    let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": alice_wallet.address.clone() }),
    )
    .await;

    let mut tx = Transaction::new(
        alice_wallet.address.clone(),
        bob_wallet.address.clone(),
        9,
        Some(b"address-pending-txs-query".to_vec()),
    );
    tx.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
        .expect("签名应成功");
    let tx_id = tx.id.clone();

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/tx",
        json!({ "transaction": tx }),
    )
    .await;

    let path = format!(
        "/chain/address/{}/pending-txs?direction=in",
        bob_wallet.address
    );
    let (status, body) = send_empty(&app, Method::GET, &path).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["returned_count"], json!(1));
    assert_eq!(body["transactions"][0]["transaction"]["id"], json!(tx_id));
}

/// 验证地址待打包交易查询支持 offset 分页。
#[tokio::test]
async fn chain_address_pending_txs_should_support_offset_pagination() {
    let app = build_test_app();
    let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
    let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": alice_wallet.address.clone() }),
    )
    .await;

    let mut tx_old = Transaction::new(
        alice_wallet.address.clone(),
        bob_wallet.address.clone(),
        4,
        Some(b"address-pending-txs-offset-old".to_vec()),
    );
    tx_old
        .sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
        .expect("签名应成功");
    let tx_old_id = tx_old.id.clone();
    let _ = send_json(
        &app,
        Method::POST,
        "/chain/tx",
        json!({ "transaction": tx_old }),
    )
    .await;

    let mut tx_new = Transaction::new(
        alice_wallet.address.clone(),
        bob_wallet.address.clone(),
        3,
        Some(b"address-pending-txs-offset-new".to_vec()),
    );
    tx_new
        .sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
        .expect("签名应成功");
    let _ = send_json(
        &app,
        Method::POST,
        "/chain/tx",
        json!({ "transaction": tx_new }),
    )
    .await;

    let path = format!(
        "/chain/address/{}/pending-txs?direction=in&limit=1&offset=1",
        bob_wallet.address
    );
    let (status, body) = send_empty(&app, Method::GET, &path).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["offset"], json!(1));
    assert_eq!(body["total_count"], json!(2));
    assert_eq!(body["returned_count"], json!(1));
    assert_eq!(
        body["transactions"][0]["transaction"]["id"],
        json!(tx_old_id)
    );
}

/// 验证待打包交易地址查询传入非法 direction 会被拒绝。
#[tokio::test]
async fn chain_address_pending_txs_with_invalid_direction_should_fail() {
    let app = build_test_app();
    let (status, body) = send_empty(
        &app,
        Method::GET,
        "/chain/address/alice/pending-txs?direction=sideways",
    )
    .await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["ok"], json!(false));
}

/// 验证可查询地址汇总（余额 + 已确认/待打包收支统计）。
#[tokio::test]
async fn chain_address_summary_should_return_counts_and_balance() {
    let app = build_test_app();
    let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
    let (bob_wallet, bob_key_pair) = create_wallet("bob-pass").expect("创建钱包应成功");

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": alice_wallet.address.clone() }),
    )
    .await;

    let mut tx_confirmed = Transaction::new(
        alice_wallet.address.clone(),
        bob_wallet.address.clone(),
        7,
        Some(b"address-summary-confirmed".to_vec()),
    );
    tx_confirmed
        .sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
        .expect("签名应成功");
    let _ = send_json(
        &app,
        Method::POST,
        "/chain/tx",
        json!({ "transaction": tx_confirmed }),
    )
    .await;
    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": "summary-miner" }),
    )
    .await;

    let mut tx_pending = Transaction::new(
        bob_wallet.address.clone(),
        alice_wallet.address.clone(),
        3,
        Some(b"address-summary-pending".to_vec()),
    );
    tx_pending
        .sign_with_private_key(&bob_key_pair.private_key, &bob_key_pair.public_key)
        .expect("签名应成功");
    let _ = send_json(
        &app,
        Method::POST,
        "/chain/tx",
        json!({ "transaction": tx_pending }),
    )
    .await;

    let path = format!("/chain/address/{}/summary", bob_wallet.address);
    let (status, body) = send_empty(&app, Method::GET, &path).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["balance"], json!(7));
    assert_eq!(body["confirmed_in_count"], json!(1));
    assert_eq!(body["confirmed_out_count"], json!(0));
    assert_eq!(body["confirmed_in_amount"], json!(7));
    assert_eq!(body["confirmed_out_amount"], json!(0));
    assert_eq!(body["pending_in_count"], json!(0));
    assert_eq!(body["pending_out_count"], json!(1));
    assert_eq!(body["pending_in_amount"], json!(0));
    assert_eq!(body["pending_out_amount"], json!(3));
}

/// 验证可按高度查询区块详情。
#[tokio::test]
async fn chain_block_by_height_should_return_block() {
    let app = build_test_app();

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": "miner-by-height" }),
    )
    .await;

    let (status, body) = send_empty(&app, Method::GET, "/chain/block/1").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["block"]["index"], json!(1));
}

/// 验证查询不存在高度时返回 404。
#[tokio::test]
async fn chain_block_by_height_not_found_should_return_404() {
    let app = build_test_app();
    let (status, body) = send_empty(&app, Method::GET, "/chain/block/99").await;
    assert_eq!(status, StatusCode::NOT_FOUND);
    assert_eq!(body["ok"], json!(false));
}

/// 验证可按区间查询区块列表。
#[tokio::test]
async fn chain_blocks_should_support_from_height_and_limit() {
    let app = build_test_app();

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": "miner-range-1" }),
    )
    .await;
    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": "miner-range-2" }),
    )
    .await;
    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": "miner-range-3" }),
    )
    .await;

    let (status, body) = send_empty(&app, Method::GET, "/chain/blocks?from_height=1&limit=2").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["returned_count"], json!(2));

    let blocks = body["blocks"].as_array().expect("blocks 应为数组");
    assert_eq!(blocks.len(), 2);
    assert_eq!(blocks[0]["index"], json!(1));
    assert_eq!(blocks[1]["index"], json!(2));
}

/// 验证区块列表查询 limit=0 会被拒绝。
#[tokio::test]
async fn chain_blocks_with_zero_limit_should_fail() {
    let app = build_test_app();
    let (status, body) = send_empty(&app, Method::GET, "/chain/blocks?limit=0").await;
    assert_eq!(status, StatusCode::BAD_REQUEST);
    assert_eq!(body["ok"], json!(false));
}

/// 验证可查询最新区块详情。
#[tokio::test]
async fn chain_latest_block_should_return_latest() {
    let app = build_test_app();

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": "latest-miner-a" }),
    )
    .await;
    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": "latest-miner-b" }),
    )
    .await;

    let (status, body) = send_empty(&app, Method::GET, "/chain/block/latest").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["block"]["index"], json!(2));
}

/// 验证链完整性校验接口可返回通过结果。
#[tokio::test]
async fn chain_validate_should_return_valid() {
    let app = build_test_app();

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": "validate-miner" }),
    )
    .await;

    let (status, body) = send_empty(&app, Method::GET, "/chain/validate").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["valid"], json!(true));
}

/// 验证 P2P 节点注册与 Ping/Pong 消息处理流程。
#[tokio::test]
async fn p2p_register_and_ping_should_work() {
    let app = build_test_app();

    let (status, body) = send_json(
        &app,
        Method::POST,
        "/p2p/peer/register",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["peer_count"], json!(1));

    let (status, body) = send_empty(&app, Method::GET, "/p2p/status").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["peer_count"], json!(1));

    let (status, body) = send_json(
        &app,
        Method::POST,
        "/p2p/message",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001",
            "sequence": 1,
            "message": {
                "Ping": {
                    "nonce": 7,
                    "timestamp": 99
                }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["processed"], json!(1));
    assert_eq!(body["outbound_count"], json!(1));
}

/// 验证长度前缀传输帧可通过 API 进入 P2P 引擎并返回出站帧。
#[tokio::test]
async fn p2p_transport_frame_should_process_complete_frame() {
    let app = build_test_app();
    let frame = FramedMessageCodec::encode_frame(&NetworkMessage::Ping {
        nonce: 17,
        timestamp: 88,
    })
    .expect("传输帧编码应成功");

    let (status, body) = send_json(
        &app,
        Method::POST,
        "/p2p/transport/frame",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001",
            "bytes": frame
        }),
    )
    .await;

    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["processed"], json!(1));
    assert_eq!(body["outbound_count"], json!(1));
    assert_eq!(body["outbound_frame_count"], json!(1));
    assert_eq!(body["session_count"], json!(1));
    assert_eq!(body["sessions"][0]["peer_id"], json!("peer-a"));
    assert_eq!(body["sessions"][0]["next_sequence"], json!(2));
}

/// 验证传输帧半包会被缓存，并在补齐后完成处理。
#[tokio::test]
async fn p2p_transport_frame_should_cache_partial_frame() {
    let app = build_test_app();
    let frame = FramedMessageCodec::encode_frame(&NetworkMessage::GetChainStatus)
        .expect("传输帧编码应成功");
    let split_at = frame.len() / 2;

    let (status, body) = send_json(
        &app,
        Method::POST,
        "/p2p/transport/frame",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001",
            "bytes": &frame[..split_at]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["processed"], json!(0));
    assert_eq!(body["sessions"][0]["buffered_bytes"], json!(split_at));

    let (status, body) = send_json(
        &app,
        Method::POST,
        "/p2p/transport/frame",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001",
            "bytes": &frame[split_at..]
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["processed"], json!(1));
    assert_eq!(body["sessions"][0]["buffered_bytes"], json!(0));

    let (status, body) = send_empty(&app, Method::GET, "/p2p/transport/sessions").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["session_count"], json!(1));
    assert_eq!(body["sessions"][0]["next_sequence"], json!(2));
}

/// 验证同步候选接口会返回按优先级排序的候选节点。
#[tokio::test]
async fn p2p_sync_candidates_should_return_sorted_candidates() {
    let app = build_test_app();
    for (peer_id, address) in [
        ("peer-a", "/ip4/127.0.0.1/tcp/7001"),
        ("peer-b", "/ip4/127.0.0.1/tcp/7002"),
    ] {
        let (status, _) = send_json(
            &app,
            Method::POST,
            "/p2p/peer/register",
            json!({
                "peer_id": peer_id,
                "address": address
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, _) = send_json(
        &app,
        Method::POST,
        "/p2p/message",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001",
            "sequence": 1,
            "message": {
                "ChainStatus": {
                    "chain_id": "rustchain-lab-dev",
                    "best_height": 12,
                    "best_hash": "0x12",
                    "difficulty": 2,
                    "genesis_hash": Block::genesis().hash
                }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = send_json(
        &app,
        Method::POST,
        "/p2p/message",
        json!({
            "peer_id": "peer-b",
            "address": "/ip4/127.0.0.1/tcp/7002",
            "sequence": 1,
            "message": {
                "ChainStatus": {
                    "chain_id": "rustchain-lab-dev",
                    "best_height": 9,
                    "best_hash": "0x9",
                    "difficulty": 2,
                    "genesis_hash": Block::genesis().hash
                }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send_empty(&app, Method::GET, "/p2p/sync-candidates").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["has_candidates"], json!(true));
    assert_eq!(body["candidate_count"], json!(2));
    assert_eq!(body["candidates"][0]["id"], json!("peer-a"));
    assert_eq!(body["candidates"][1]["id"], json!("peer-b"));
}

/// 验证同步目标接口会返回最优候选节点。
#[tokio::test]
async fn p2p_sync_target_should_return_best_candidate() {
    let app = build_test_app();
    for (peer_id, address) in [
        ("peer-a", "/ip4/127.0.0.1/tcp/7001"),
        ("peer-b", "/ip4/127.0.0.1/tcp/7002"),
        ("peer-c", "/ip4/127.0.0.1/tcp/7003"),
    ] {
        let (status, _) = send_json(
            &app,
            Method::POST,
            "/p2p/peer/register",
            json!({
                "peer_id": peer_id,
                "address": address
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, _) = send_json(
        &app,
        Method::POST,
        "/p2p/message",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001",
            "sequence": 1,
            "message": {
                "ChainStatus": {
                    "chain_id": "rustchain-lab-dev",
                    "best_height": 12,
                    "best_hash": "0x12",
                    "difficulty": 2,
                    "genesis_hash": Block::genesis().hash
                }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = send_json(
        &app,
        Method::POST,
        "/p2p/message",
        json!({
            "peer_id": "peer-b",
            "address": "/ip4/127.0.0.1/tcp/7002",
            "sequence": 1,
            "message": {
                "ChainStatus": {
                    "chain_id": "rustchain-lab-dev",
                    "best_height": 10,
                    "best_hash": "0x10",
                    "difficulty": 2,
                    "genesis_hash": Block::genesis().hash
                }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send_empty(&app, Method::GET, "/p2p/sync-target").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["has_target"], json!(true));
    assert_eq!(body["target"]["id"], json!("peer-a"));
}

/// 验证同步计划接口会返回可执行的 GetBlocks 请求。
#[tokio::test]
async fn p2p_sync_plan_should_return_get_blocks_plan() {
    let app = build_test_app();

    let (status, _) = send_json(
        &app,
        Method::POST,
        "/p2p/peer/register",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = send_json(
        &app,
        Method::POST,
        "/p2p/message",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001",
            "sequence": 1,
            "message": {
                "ChainStatus": {
                    "chain_id": "rustchain-lab-dev",
                    "best_height": 9,
                    "best_hash": "0x9",
                    "difficulty": 2,
                    "genesis_hash": Block::genesis().hash
                }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send_empty(&app, Method::GET, "/p2p/sync-plan").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["has_target"], json!(true));
    assert_eq!(body["has_plan"], json!(true));
    assert_eq!(body["target"]["id"], json!("peer-a"));
    assert_eq!(body["plan"]["GetBlocks"]["from_height"], json!(1));
    assert_eq!(body["plan"]["GetBlocks"]["limit"], json!(9));
}

/// 验证同步差距接口会返回高度差和批次数估算。
#[tokio::test]
async fn p2p_sync_gap_should_return_gap_and_estimated_batches() {
    let app = build_test_app();

    let (status, _) = send_json(
        &app,
        Method::POST,
        "/p2p/peer/register",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = send_json(
        &app,
        Method::POST,
        "/p2p/message",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001",
            "sequence": 1,
            "message": {
                "ChainStatus": {
                    "chain_id": "rustchain-lab-dev",
                    "best_height": 300,
                    "best_hash": "0x300",
                    "difficulty": 2,
                    "genesis_hash": Block::genesis().hash
                }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send_empty(&app, Method::GET, "/p2p/sync-gap").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["has_target"], json!(true));
    assert_eq!(body["target"]["id"], json!("peer-a"));
    assert_eq!(body["local_height"], json!(0));
    assert_eq!(body["target_height"], json!(300));
    assert_eq!(body["gap"], json!(300));
    assert_eq!(body["estimated_batches"], json!(3));
    assert_eq!(body["next_request"]["GetBlocks"]["from_height"], json!(1));
    assert_eq!(body["next_request"]["GetBlocks"]["limit"], json!(128));
}

/// 验证同步单步接口会返回可直接执行的拉块动作。
#[tokio::test]
async fn p2p_sync_step_should_return_action() {
    let app = build_test_app();

    let (status, _) = send_json(
        &app,
        Method::POST,
        "/p2p/peer/register",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, _) = send_json(
        &app,
        Method::POST,
        "/p2p/message",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001",
            "sequence": 1,
            "message": {
                "ChainStatus": {
                    "chain_id": "rustchain-lab-dev",
                    "best_height": 6,
                    "best_hash": "0x6",
                    "difficulty": 2,
                    "genesis_hash": Block::genesis().hash
                }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send_empty(&app, Method::GET, "/p2p/sync-step").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["has_action"], json!(true));
    assert_eq!(body["target"]["id"], json!("peer-a"));
    assert_eq!(body["action"]["target_peer_id"], json!("peer-a"));
    assert_eq!(
        body["action"]["message"]["GetBlocks"]["from_height"],
        json!(1)
    );
    assert_eq!(body["action"]["message"]["GetBlocks"]["limit"], json!(6));
}

/// 验证 P2P 最近邻查询接口可用并返回 limit 条记录。
#[tokio::test]
async fn p2p_nearest_peers_should_work() {
    let app = build_test_app();
    for (peer_id, address) in [
        ("peer-a", "/ip4/127.0.0.1/tcp/7001"),
        ("peer-b", "/ip4/127.0.0.1/tcp/7002"),
        ("peer-c", "/ip4/127.0.0.1/tcp/7003"),
    ] {
        let (status, _) = send_json(
            &app,
            Method::POST,
            "/p2p/peer/register",
            json!({
                "peer_id": peer_id,
                "address": address
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, body) = send_empty(
        &app,
        Method::GET,
        "/p2p/peers/nearest?target_peer_id=target-1&limit=2",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    let peers = body["peers"].as_array().expect("peers 应为数组");
    assert_eq!(peers.len(), 2);
}

/// 验证 P2P DHT 桶视图查询接口可用。
#[tokio::test]
async fn p2p_dht_buckets_should_work() {
    let app = build_test_app();
    for (peer_id, address) in [
        ("peer-a", "/ip4/127.0.0.1/tcp/7001"),
        ("peer-b", "/ip4/127.0.0.1/tcp/7002"),
        ("peer-c", "/ip4/127.0.0.1/tcp/7003"),
    ] {
        let (status, _) = send_json(
            &app,
            Method::POST,
            "/p2p/peer/register",
            json!({
                "peer_id": peer_id,
                "address": address
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, body) = send_empty(
        &app,
        Method::GET,
        "/p2p/dht/buckets?target_peer_id=target-2&bucket_count=8",
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    let buckets = body["buckets"].as_array().expect("buckets 应为数组");
    assert!(!buckets.is_empty());
}

/// 验证 P2P find_node 会返回 nodes 结果。
#[tokio::test]
async fn p2p_find_node_should_return_nodes() {
    let app = build_test_app();
    for (peer_id, address) in [
        ("peer-a", "/ip4/127.0.0.1/tcp/7001"),
        ("peer-b", "/ip4/127.0.0.1/tcp/7002"),
        ("peer-c", "/ip4/127.0.0.1/tcp/7003"),
    ] {
        let (status, _) = send_json(
            &app,
            Method::POST,
            "/p2p/peer/register",
            json!({
                "peer_id": peer_id,
                "address": address
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, body) = send_json(
        &app,
        Method::POST,
        "/p2p/message",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001",
            "sequence": 1,
            "message": {
                "FindNode": {
                    "target_id": "target-3",
                    "limit": 2
                }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["outbound_count"], json!(1));
    let peers = body["outbound"][0]["message"]["Nodes"]["peers"]
        .as_array()
        .expect("nodes.peers 应为数组");
    assert_eq!(peers.len(), 2);
}

/// 验证接收 nodes 消息后会导入发现节点。
#[tokio::test]
async fn p2p_nodes_should_import_discovered_peers() {
    let app = build_test_app();
    let (status, _) = send_json(
        &app,
        Method::POST,
        "/p2p/peer/register",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send_json(
        &app,
        Method::POST,
        "/p2p/message",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001",
            "sequence": 1,
            "message": {
                "Nodes": {
                    "peers": [
                        {
                            "peer_id": "peer-b",
                            "address": "/ip4/127.0.0.1/tcp/7002"
                        },
                        {
                            "peer_id": "peer-c",
                            "address": "/ip4/127.0.0.1/tcp/7003"
                        }
                    ]
                }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));

    let (status, body) = send_empty(&app, Method::GET, "/p2p/peers").await;
    assert_eq!(status, StatusCode::OK);
    let peers = body["peers"].as_array().expect("peers 应为数组");
    assert_eq!(peers.len(), 3);

    let (status, body) = send_empty(&app, Method::GET, "/chain/info").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["chain"]["peer_count"], json!(3));
}

/// 验证 P2P 启动引导会仅返回待握手节点。
#[tokio::test]
async fn p2p_bootstrap_should_only_include_unconnected_peers() {
    let app = build_test_app();
    for (peer_id, address) in [
        ("peer-a", "/ip4/127.0.0.1/tcp/7001"),
        ("peer-b", "/ip4/127.0.0.1/tcp/7002"),
    ] {
        let (status, _) = send_json(
            &app,
            Method::POST,
            "/p2p/peer/register",
            json!({
                "peer_id": peer_id,
                "address": address
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    // 通过一次消息交互将 peer-a 标记为已连接。
    let (status, _) = send_json(
        &app,
        Method::POST,
        "/p2p/message",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001",
            "sequence": 1,
            "message": "GetChainStatus"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send_json(&app, Method::POST, "/p2p/bootstrap", json!({})).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["outbound_count"], json!(1));
    assert_eq!(body["outbound"][0]["target_peer_id"], json!("peer-b"));
    assert_eq!(
        body["outbound"][0]["message"]["Handshake"]["protocol_version"],
        json!("1.0.0")
    );
}

/// 验证 P2P 批量发现接口会构建 find_node 请求。
#[tokio::test]
async fn p2p_discover_should_build_find_node_requests() {
    let app = build_test_app();
    for (peer_id, address) in [
        ("peer-a", "/ip4/127.0.0.1/tcp/7001"),
        ("peer-b", "/ip4/127.0.0.1/tcp/7002"),
    ] {
        let (status, _) = send_json(
            &app,
            Method::POST,
            "/p2p/peer/register",
            json!({
                "peer_id": peer_id,
                "address": address
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, body) = send_json(
        &app,
        Method::POST,
        "/p2p/discover",
        json!({
            "target_peer_id": "target-discover",
            "limit": 3
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["outbound_count"], json!(2));
    assert_eq!(
        body["outbound"][0]["message"]["FindNode"]["target_id"],
        json!("target-discover")
    );
    assert_eq!(
        body["outbound"][0]["message"]["FindNode"]["limit"],
        json!(3)
    );
}

/// 验证 P2P 诊断接口会返回状态摘要与建议动作。
#[tokio::test]
async fn p2p_diagnose_should_return_summary_and_suggestions() {
    let app = build_test_app();
    for (peer_id, address) in [
        ("peer-a", "/ip4/127.0.0.1/tcp/7001"),
        ("peer-b", "/ip4/127.0.0.1/tcp/7002"),
    ] {
        let (status, _) = send_json(
            &app,
            Method::POST,
            "/p2p/peer/register",
            json!({
                "peer_id": peer_id,
                "address": address
            }),
        )
        .await;
        assert_eq!(status, StatusCode::OK);
    }

    let (status, body) = send_json(
        &app,
        Method::POST,
        "/p2p/diagnose",
        json!({
            "target_peer_id": "target-diagnose",
            "limit": 2
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["peer_count"], json!(2));
    assert_eq!(body["discover_outbound_count"], json!(2));
    assert_eq!(body["nearest_peer_count"], json!(2));
    let suggestions = body["suggestions"]
        .as_array()
        .expect("suggestions 应为数组");
    assert!(!suggestions.is_empty());
}

/// 验证链交易提交与挖矿会触发 P2P 广播。
#[tokio::test]
async fn chain_actions_should_broadcast_to_connected_peers() {
    let app = build_test_app();
    let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
    let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

    let _ = send_json(
        &app,
        Method::POST,
        "/p2p/peer/register",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001"
        }),
    )
    .await;
    let _ = send_json(
        &app,
        Method::POST,
        "/p2p/message",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001",
            "sequence": 1,
            "message": "GetChainStatus"
        }),
    )
    .await;

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": alice_wallet.address.clone() }),
    )
    .await;

    let mut tx = Transaction::new(
        alice_wallet.address.clone(),
        bob_wallet.address.clone(),
        10,
        Some(b"api-p2p-broadcast".to_vec()),
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
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["p2p_outbound_count"], json!(1));

    let (status, body) = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": "miner-2" }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["p2p_outbound_count"], json!(1));
}

/// 验证 P2P GetBlocks 会返回真实区块数据。
#[tokio::test]
async fn p2p_get_blocks_should_return_real_blocks() {
    let app = build_test_app();
    let (miner_wallet, _) = create_wallet("miner-pass").expect("创建钱包应成功");

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": miner_wallet.address.clone() }),
    )
    .await;
    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": miner_wallet.address.clone() }),
    )
    .await;

    let (status, body) = send_json(
        &app,
        Method::POST,
        "/p2p/message",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001",
            "sequence": 1,
            "message": {
                "GetBlocks": {
                    "from_height": 1,
                    "limit": 10
                }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["outbound_count"], json!(1));
    let blocks = body["outbound"][0]["message"]["Blocks"]["blocks"]
        .as_array()
        .expect("blocks 应为数组");
    assert_eq!(blocks.len(), 2);
}

/// 验证 P2P NewTransaction 会导入本地交易池。
#[tokio::test]
async fn p2p_new_transaction_should_import_to_mempool() {
    let app = build_test_app();
    let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
    let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": alice_wallet.address.clone() }),
    )
    .await;

    let mut tx = Transaction::new(
        alice_wallet.address.clone(),
        bob_wallet.address.clone(),
        5,
        Some(b"p2p-sync-tx".to_vec()),
    );
    tx.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
        .expect("签名应成功");
    let tx_bytes = bincode::serialize(&tx).expect("交易编码应成功");

    let (status, body) = send_json(
        &app,
        Method::POST,
        "/p2p/message",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001",
            "sequence": 1,
            "message": {
                "NewTransaction": {
                    "transaction": tx_bytes
                }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));

    let (status, body) = send_empty(&app, Method::GET, "/chain/info").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["chain"]["pending_tx_count"], json!(1));
}

/// 验证 P2P GetMempool 会返回真实交易池快照。
#[tokio::test]
async fn p2p_get_mempool_should_return_real_transactions() {
    let app = build_test_app();
    let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
    let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": alice_wallet.address.clone() }),
    )
    .await;

    let mut tx = Transaction::new(
        alice_wallet.address.clone(),
        bob_wallet.address.clone(),
        6,
        Some(b"p2p-get-mempool".to_vec()),
    );
    tx.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
        .expect("签名应成功");
    let tx_id = tx.id.clone();
    let _ = send_json(
        &app,
        Method::POST,
        "/chain/tx",
        json!({ "transaction": tx }),
    )
    .await;

    let (status, body) = send_json(
        &app,
        Method::POST,
        "/p2p/message",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001",
            "sequence": 1,
            "message": "GetMempool"
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["outbound_count"], json!(1));

    let tx_list = body["outbound"][0]["message"]["Mempool"]["transactions"]
        .as_array()
        .expect("transactions 应为数组");
    assert_eq!(tx_list.len(), 1);

    let raw: Vec<u8> = serde_json::from_value(tx_list[0].clone()).expect("交易字节应可反序列化");
    let decoded_tx: Transaction = bincode::deserialize(&raw).expect("交易应可反序列化");
    assert_eq!(decoded_tx.id, tx_id);
}

/// 验证 P2P Mempool 会批量导入交易到本地交易池。
#[tokio::test]
async fn p2p_mempool_should_import_transactions() {
    let app = build_test_app();
    let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
    let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": alice_wallet.address.clone() }),
    )
    .await;

    let mut tx = Transaction::new(
        alice_wallet.address.clone(),
        bob_wallet.address.clone(),
        7,
        Some(b"p2p-import-mempool".to_vec()),
    );
    tx.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
        .expect("签名应成功");
    let tx_bytes = bincode::serialize(&tx).expect("交易编码应成功");

    let (status, body) = send_json(
        &app,
        Method::POST,
        "/p2p/message",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001",
            "sequence": 1,
            "message": {
                "Mempool": {
                    "transactions": [tx_bytes]
                }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));

    let (status, body) = send_empty(&app, Method::GET, "/chain/info").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["chain"]["pending_tx_count"], json!(1));
}

/// 验证 P2P NewBlock 会同步并追加本地区块。
#[tokio::test]
async fn p2p_new_block_should_sync_chain() {
    let app = build_test_app();
    let mut remote_chain = Blockchain::new(2, 50);
    let block = remote_chain
        .mine_pending_transactions("remote-miner")
        .expect("远端出块应成功");
    let block_bytes = bincode::serialize(&block).expect("区块编码应成功");

    let (status, body) = send_json(
        &app,
        Method::POST,
        "/p2p/message",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001",
            "sequence": 1,
            "message": {
                "NewBlock": {
                    "block": block_bytes
                }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));

    let (status, body) = send_empty(&app, Method::GET, "/chain/info").await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["chain"]["height"], json!(1));
}

/// 验证接收 Blocks 后若仍落后，会继续请求下一批区块。
#[tokio::test]
async fn p2p_blocks_should_request_next_batch_when_still_behind() {
    let app = build_test_app();
    let mut remote_chain = Blockchain::new(2, 50);
    let block = remote_chain
        .mine_pending_transactions("remote-miner")
        .expect("远端出块应成功");
    let block_bytes = bincode::serialize(&block).expect("区块编码应成功");

    let (status, _) = send_json(
        &app,
        Method::POST,
        "/p2p/message",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001",
            "sequence": 1,
            "message": {
                "ChainStatus": {
                    "chain_id": "rustchain-lab-dev",
                    "best_height": 5,
                    "best_hash": "remote-tip-5",
                    "difficulty": 2,
                    "genesis_hash": Block::genesis().hash
                }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);

    let (status, body) = send_json(
        &app,
        Method::POST,
        "/p2p/message",
        json!({
            "peer_id": "peer-a",
            "address": "/ip4/127.0.0.1/tcp/7001",
            "sequence": 2,
            "message": {
                "Blocks": {
                    "blocks": [block_bytes]
                }
            }
        }),
    )
    .await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["outbound_count"], json!(1));
    assert_eq!(
        body["outbound"][0]["message"]["GetBlocks"]["from_height"],
        json!(2)
    );
    assert_eq!(
        body["outbound"][0]["message"]["GetBlocks"]["limit"],
        json!(4)
    );
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

/// 验证统一交易查询会优先命中待打包交易。
#[tokio::test]
async fn chain_tx_query_should_return_pending_first() {
    let app = build_test_app();
    let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
    let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": alice_wallet.address.clone() }),
    )
    .await;

    let mut tx = Transaction::new(
        alice_wallet.address.clone(),
        bob_wallet.address.clone(),
        6,
        Some(b"chain-tx-query-pending".to_vec()),
    );
    tx.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
        .expect("签名应成功");
    let tx_id = tx.id.clone();

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/tx",
        json!({ "transaction": tx }),
    )
    .await;

    let path = format!("/chain/tx/{tx_id}");
    let (status, body) = send_empty(&app, Method::GET, &path).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["source"], json!("pending"));
    assert_eq!(body["transaction"]["id"], json!(tx_id));
}

/// 验证统一交易查询可回退命中历史交易。
#[tokio::test]
async fn chain_tx_query_should_fallback_to_history() {
    let app = build_test_app();
    let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应成功");
    let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应成功");

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": alice_wallet.address.clone() }),
    )
    .await;

    let mut tx = Transaction::new(
        alice_wallet.address.clone(),
        bob_wallet.address.clone(),
        4,
        Some(b"chain-tx-query-history".to_vec()),
    );
    tx.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
        .expect("签名应成功");
    let tx_id = tx.id.clone();

    let _ = send_json(
        &app,
        Method::POST,
        "/chain/tx",
        json!({ "transaction": tx }),
    )
    .await;
    let _ = send_json(
        &app,
        Method::POST,
        "/chain/mine",
        json!({ "miner_address": "history-miner" }),
    )
    .await;

    let path = format!("/chain/tx/{tx_id}");
    let (status, body) = send_empty(&app, Method::GET, &path).await;
    assert_eq!(status, StatusCode::OK);
    assert_eq!(body["ok"], json!(true));
    assert_eq!(body["source"], json!("history"));
    assert_eq!(body["transaction"]["id"], json!(tx_id));
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
async fn send_json(app: &Router, method: Method, uri: &str, payload: Value) -> (StatusCode, Value) {
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

/// 发送空载荷请求并读取文本响应。
async fn send_text(app: &Router, method: Method, uri: &str) -> (StatusCode, String) {
    let request = Request::builder()
        .method(method)
        .uri(uri)
        .body(Body::empty())
        .expect("请求构造应成功");
    let response = app.clone().oneshot(request).await.expect("请求处理应成功");
    let status = response.status();
    let bytes = to_bytes(response.into_body(), usize::MAX)
        .await
        .expect("读取响应体应成功");
    let body = String::from_utf8(bytes.to_vec()).expect("响应应为 UTF-8 文本");
    (status, body)
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
