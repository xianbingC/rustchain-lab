use reqwest::{blocking::Client, Method};
use rustchain_common::{logging::init_logging, AppConfig, AppError, AppResult};
use rustchain_core::block::Block;
use rustchain_core::transaction::{Transaction, TransactionKind};
use rustchain_crypto::wallet::create_wallet;
use serde_json::{json, Value};
use std::{
    fs,
    time::{SystemTime, UNIX_EPOCH},
};

/// CLI 程序入口。
fn main() {
    if let Err(error) = run() {
        eprintln!("CLI 执行失败: {error}");
        std::process::exit(1);
    }
}

/// 执行 CLI 初始化和命令分发流程。
fn run() -> AppResult<()> {
    let config = AppConfig::from_env("rustchain-cli")?;
    init_logging(&config)?;

    tracing::info!(
        app = %config.app_name,
        data_dir = %config.data_dir,
        difficulty = config.mining_difficulty,
        reward = config.mining_reward,
        "CLI 初始化完成"
    );

    let args = std::env::args().skip(1).collect::<Vec<_>>();
    dispatch_command(&config, &args)
}

/// 按参数分发命令。
fn dispatch_command(config: &AppConfig, args: &[String]) -> AppResult<()> {
    if args.is_empty() {
        print_help();
        return Ok(());
    }

    match args[0].as_str() {
        "wallet" => handle_wallet_command(args),
        "tx" => handle_tx_command(args),
        "chain" => handle_chain_command(config, args),
        "p2p" => handle_p2p_command(config, args),
        "vm" => handle_vm_command(config, args),
        "defi" => handle_defi_command(config, args),
        "nft" => handle_nft_command(config, args),
        "help" | "--help" | "-h" => {
            print_help();
            Ok(())
        }
        other => Err(AppError::Command(format!("未知命令: {other}"))),
    }
}

/// 处理钱包相关命令。
fn handle_wallet_command(args: &[String]) -> AppResult<()> {
    if args.len() < 2 {
        return Err(AppError::Command(
            "wallet 命令缺少子命令，可用: wallet create <password>".to_string(),
        ));
    }

    match args[1].as_str() {
        "create" => {
            if args.len() < 3 {
                return Err(AppError::Command("wallet create 需要密码参数".to_string()));
            }
            let password = &args[2];
            let (wallet, key_pair) = create_wallet(password)
                .map_err(|error| AppError::Command(format!("创建钱包失败: {error}")))?;

            print_json(
                "wallet",
                json!({
                    "address": wallet.address,
                    "public_key": wallet.public_key,
                    "encrypted_private_key": wallet.encrypted_private_key,
                    "kdf_salt": wallet.kdf_salt,
                }),
            )?;

            // 原型阶段用于学习和调试，演示时直接输出私钥。
            print_json(
                "wallet_key_pair",
                json!({
                    "address": key_pair.address,
                    "public_key": key_pair.public_key,
                    "private_key": key_pair.private_key,
                }),
            )
        }
        other => Err(AppError::Command(format!("未知 wallet 子命令: {other}"))),
    }
}

/// 处理交易相关命令。
fn handle_tx_command(args: &[String]) -> AppResult<()> {
    if args.len() < 2 {
        return Err(AppError::Command(
            "tx 命令缺少子命令，可用: tx sign-demo [amount]".to_string(),
        ));
    }

    match args[1].as_str() {
        "sign-demo" => run_sign_demo(args),
        other => Err(AppError::Command(format!("未知 tx 子命令: {other}"))),
    }
}

/// 执行签名演示：创建两个钱包、生成交易、签名并校验。
fn run_sign_demo(args: &[String]) -> AppResult<()> {
    let amount = if args.len() >= 3 {
        args[2]
            .parse::<u64>()
            .map_err(|error| AppError::Command(format!("amount 参数解析失败: {error}")))?
    } else {
        10
    };

    if amount == 0 {
        return Err(AppError::Command("amount 必须大于 0".to_string()));
    }

    let (sender_wallet, sender_key_pair) =
        create_wallet("sender-pass").map_err(|error| AppError::Command(error.to_string()))?;
    let (receiver_wallet, _) =
        create_wallet("receiver-pass").map_err(|error| AppError::Command(error.to_string()))?;

    let mut tx = Transaction::new(
        sender_wallet.address.clone(),
        receiver_wallet.address.clone(),
        amount,
        Some(b"cli-sign-demo".to_vec()),
    );
    tx.sign_with_private_key(&sender_key_pair.private_key, &sender_key_pair.public_key)
        .map_err(|error| AppError::Command(format!("交易签名失败: {error}")))?;
    tx.validate_for_chain()
        .map_err(|error| AppError::Command(format!("交易校验失败: {error}")))?;

    print_json(
        "tx_sign_demo",
        json!({
            "from": tx.from,
            "to": tx.to,
            "amount": tx.amount,
            "nonce": tx.nonce,
            "id": tx.id,
            "signature": tx.signature,
            "sender_public_key": tx.sender_public_key,
            "validated": true
        }),
    )
}

/// 处理链核心相关命令（通过 API 调用）。
fn handle_chain_command(config: &AppConfig, args: &[String]) -> AppResult<()> {
    if args.len() < 2 {
        return Err(AppError::Command(
            "chain 命令缺少子命令，可用: info/balance/contract-state/contract-events/contract-field/mine/transfer/contract-call-file/history-block/history-tx"
                .to_string(),
        ));
    }

    match args[1].as_str() {
        "info" => {
            let response = call_api_json(config, Method::GET, "/chain/info", None)?;
            print_json("chain_info", response)
        }
        "balance" => {
            let address = require_arg(args, 2, "address")?;
            let path = format!("/chain/balance/{address}");
            let response = call_api_json(config, Method::GET, &path, None)?;
            print_json("chain_balance", response)
        }
        "contract-state" => {
            let address = require_arg(args, 2, "address")?;
            let path = format!("/chain/contract/{address}/state");
            let response = call_api_json(config, Method::GET, &path, None)?;
            print_json("chain_contract_state", response)
        }
        "contract-events" => {
            let address = require_arg(args, 2, "address")?;
            let path = format!("/chain/contract/{address}/events");
            let response = call_api_json(config, Method::GET, &path, None)?;
            print_json("chain_contract_events", response)
        }
        "contract-field" => {
            let address = require_arg(args, 2, "address")?;
            let field = require_arg(args, 3, "field")?;
            let path = format!("/chain/contract/{address}/field/{field}");
            let response = call_api_json(config, Method::GET, &path, None)?;
            print_json("chain_contract_field", response)
        }
        "mine" => {
            let miner_address = require_arg(args, 2, "miner_address")?;
            let response = call_api_json(
                config,
                Method::POST,
                "/chain/mine",
                Some(json!({ "miner_address": miner_address })),
            )?;
            print_json("chain_mine", response)
        }
        "transfer" => {
            let from = require_arg(args, 2, "from")?;
            let to = require_arg(args, 3, "to")?;
            let amount = parse_u64_arg(args, 4, "amount")?;
            let private_key = require_arg(args, 5, "private_key")?;
            let public_key = require_arg(args, 6, "public_key")?;
            let payload = args
                .get(7)
                .map(|text| text.as_bytes().to_vec())
                .or_else(|| Some(b"cli-chain-transfer".to_vec()));

            let tx = build_signed_transfer_tx(from, to, amount, private_key, public_key, payload)?;
            let response = call_api_json(
                config,
                Method::POST,
                "/chain/tx",
                Some(json!({ "transaction": tx })),
            )?;
            print_json("chain_transfer_submit", response)
        }
        "contract-call-file" => {
            let from = require_arg(args, 2, "from")?;
            let to = require_arg(args, 3, "to")?;
            let amount = parse_u64_arg(args, 4, "amount")?;
            let private_key = require_arg(args, 5, "private_key")?;
            let public_key = require_arg(args, 6, "public_key")?;
            let source_path = require_arg(args, 7, "source_path")?;
            let nonce = args
                .get(8)
                .map(|raw| {
                    raw.parse::<u64>()
                        .map_err(|error| AppError::Command(format!("nonce 参数解析失败: {error}")))
                })
                .transpose()?
                .unwrap_or(0);
            let payload = read_contract_source(source_path)?.into_bytes();

            let tx = build_signed_contract_call_tx(
                from,
                to,
                amount,
                nonce,
                private_key,
                public_key,
                payload,
            )?;
            let response = call_api_json(
                config,
                Method::POST,
                "/chain/tx",
                Some(json!({ "transaction": tx })),
            )?;
            print_json("chain_contract_call_submit", response)
        }
        "history-block" => {
            let block_hash = require_arg(args, 2, "block_hash")?;
            let path = format!("/history/block/{block_hash}");
            let response = call_api_json(config, Method::GET, &path, None)?;
            print_json("chain_history_block", response)
        }
        "history-tx" => {
            let tx_id = require_arg(args, 2, "tx_id")?;
            let path = format!("/history/tx/{tx_id}");
            let response = call_api_json(config, Method::GET, &path, None)?;
            print_json("chain_history_tx", response)
        }
        other => Err(AppError::Command(format!("未知 chain 子命令: {other}"))),
    }
}

/// 处理 P2P 相关命令（通过 API 调用）。
fn handle_p2p_command(config: &AppConfig, args: &[String]) -> AppResult<()> {
    if args.len() < 2 {
        return Err(AppError::Command(
            "p2p 命令缺少子命令，可用: status/peers/register-peer/ping/get-chain-status/chain-status/get-blocks"
                .to_string(),
        ));
    }

    match args[1].as_str() {
        "status" => {
            let response = call_api_json(config, Method::GET, "/p2p/status", None)?;
            print_json("p2p_status", response)
        }
        "peers" => {
            let response = call_api_json(config, Method::GET, "/p2p/peers", None)?;
            print_json("p2p_peers", response)
        }
        "register-peer" => {
            let peer_id = require_arg(args, 2, "peer_id")?;
            let address = require_arg(args, 3, "address")?;
            let response = call_api_json(
                config,
                Method::POST,
                "/p2p/peer/register",
                Some(json!({
                    "peer_id": peer_id,
                    "address": address
                })),
            )?;
            print_json("p2p_register_peer", response)
        }
        "ping" => {
            let peer_id = require_arg(args, 2, "peer_id")?;
            let address = require_arg(args, 3, "address")?;
            let sequence = parse_u64_arg(args, 4, "sequence")?;
            let nonce = args
                .get(5)
                .map(|raw| {
                    raw.parse::<u64>()
                        .map_err(|error| AppError::Command(format!("nonce 参数解析失败: {error}")))
                })
                .transpose()?
                .unwrap_or(sequence);
            let timestamp = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|duration| duration.as_secs() as i64)
                .unwrap_or(0);

            let response = call_api_json(
                config,
                Method::POST,
                "/p2p/message",
                Some(json!({
                    "peer_id": peer_id,
                    "address": address,
                    "sequence": sequence,
                    "message": {
                        "Ping": {
                            "nonce": nonce,
                            "timestamp": timestamp
                        }
                    }
                })),
            )?;
            print_json("p2p_ping", response)
        }
        "get-chain-status" => {
            let peer_id = require_arg(args, 2, "peer_id")?;
            let address = require_arg(args, 3, "address")?;
            let sequence = parse_u64_arg(args, 4, "sequence")?;
            let response = call_api_json(
                config,
                Method::POST,
                "/p2p/message",
                Some(json!({
                    "peer_id": peer_id,
                    "address": address,
                    "sequence": sequence,
                    "message": "GetChainStatus"
                })),
            )?;
            print_json("p2p_get_chain_status", response)
        }
        "chain-status" => {
            let peer_id = require_arg(args, 2, "peer_id")?;
            let address = require_arg(args, 3, "address")?;
            let sequence = parse_u64_arg(args, 4, "sequence")?;
            let best_height = parse_u64_arg(args, 5, "best_height")?;
            let best_hash = require_arg(args, 6, "best_hash")?;
            let difficulty = args
                .get(7)
                .map(|raw| {
                    raw.parse::<u32>().map_err(|error| {
                        AppError::Command(format!("difficulty 参数解析失败: {error}"))
                    })
                })
                .transpose()?
                .unwrap_or(config.mining_difficulty);
            let response = call_api_json(
                config,
                Method::POST,
                "/p2p/message",
                Some(json!({
                    "peer_id": peer_id,
                    "address": address,
                    "sequence": sequence,
                    "message": {
                        "ChainStatus": {
                            "chain_id": "rustchain-lab-dev",
                            "best_height": best_height,
                            "best_hash": best_hash,
                            "difficulty": difficulty,
                            "genesis_hash": Block::genesis().hash
                        }
                    }
                })),
            )?;
            print_json("p2p_chain_status", response)
        }
        "get-blocks" => {
            let peer_id = require_arg(args, 2, "peer_id")?;
            let address = require_arg(args, 3, "address")?;
            let sequence = parse_u64_arg(args, 4, "sequence")?;
            let from_height = parse_u64_arg(args, 5, "from_height")?;
            let limit = args
                .get(6)
                .map(|raw| {
                    raw.parse::<u32>()
                        .map_err(|error| AppError::Command(format!("limit 参数解析失败: {error}")))
                })
                .transpose()?
                .unwrap_or(128);
            let response = call_api_json(
                config,
                Method::POST,
                "/p2p/message",
                Some(json!({
                    "peer_id": peer_id,
                    "address": address,
                    "sequence": sequence,
                    "message": {
                        "GetBlocks": {
                            "from_height": from_height,
                            "limit": limit
                        }
                    }
                })),
            )?;
            print_json("p2p_get_blocks", response)
        }
        other => Err(AppError::Command(format!("未知 p2p 子命令: {other}"))),
    }
}

/// 处理 VM 相关命令（通过 API 调用）。
fn handle_vm_command(config: &AppConfig, args: &[String]) -> AppResult<()> {
    if args.len() < 2 {
        return Err(AppError::Command(
            "vm 命令缺少子命令，可用: compile-file/execute-file".to_string(),
        ));
    }

    match args[1].as_str() {
        "compile-file" => {
            let source_path = require_arg(args, 2, "source_path")?;
            let source = read_contract_source(source_path)?;
            let response = call_api_json(
                config,
                Method::POST,
                "/vm/compile",
                Some(json!({ "source": source })),
            )?;
            print_json("vm_compile", response)
        }
        "execute-file" => {
            let source_path = require_arg(args, 2, "source_path")?;
            let source = read_contract_source(source_path)?;
            let max_steps = args
                .get(3)
                .map(|raw| {
                    raw.parse::<usize>().map_err(|error| {
                        AppError::Command(format!("max_steps 参数解析失败: {error}"))
                    })
                })
                .transpose()?;

            if matches!(max_steps, Some(0)) {
                return Err(AppError::Command("max_steps 必须大于 0".to_string()));
            }

            let mut payload = json!({ "source": source });
            if let Some(max_steps) = max_steps {
                payload["max_steps"] = json!(max_steps);
            }

            let response = call_api_json(config, Method::POST, "/vm/execute", Some(payload))?;
            print_json("vm_execute", response)
        }
        other => Err(AppError::Command(format!("未知 vm 子命令: {other}"))),
    }
}

/// 处理 DeFi 相关命令（通过 API 调用）。
fn handle_defi_command(config: &AppConfig, args: &[String]) -> AppResult<()> {
    if args.len() < 2 {
        return Err(AppError::Command(
            "defi 命令缺少子命令，可用: deposit/borrow/repay/withdraw/liquidate/position/stats"
                .to_string(),
        ));
    }

    match args[1].as_str() {
        "deposit" => {
            let owner = require_arg(args, 2, "owner")?;
            let amount = parse_u64_arg(args, 3, "amount")?;
            let response = call_api_json(
                config,
                Method::POST,
                "/defi/deposit",
                Some(json!({ "owner": owner, "amount": amount })),
            )?;
            print_json("defi_deposit", response)
        }
        "borrow" => {
            let owner = require_arg(args, 2, "owner")?;
            let amount = parse_u64_arg(args, 3, "amount")?;
            let response = call_api_json(
                config,
                Method::POST,
                "/defi/borrow",
                Some(json!({ "owner": owner, "amount": amount })),
            )?;
            print_json("defi_borrow", response)
        }
        "repay" => {
            let owner = require_arg(args, 2, "owner")?;
            let amount = parse_u64_arg(args, 3, "amount")?;
            let response = call_api_json(
                config,
                Method::POST,
                "/defi/repay",
                Some(json!({ "owner": owner, "amount": amount })),
            )?;
            print_json("defi_repay", response)
        }
        "withdraw" => {
            let owner = require_arg(args, 2, "owner")?;
            let amount = parse_u64_arg(args, 3, "amount")?;
            let response = call_api_json(
                config,
                Method::POST,
                "/defi/withdraw",
                Some(json!({ "owner": owner, "amount": amount })),
            )?;
            print_json("defi_withdraw", response)
        }
        "liquidate" => {
            let borrower = require_arg(args, 2, "borrower")?;
            let amount = parse_u64_arg(args, 3, "amount")?;
            let response = call_api_json(
                config,
                Method::POST,
                "/defi/liquidate",
                Some(json!({ "borrower": borrower, "amount": amount })),
            )?;
            print_json("defi_liquidate", response)
        }
        "position" => {
            let owner = require_arg(args, 2, "owner")?;
            let path = format!("/defi/position/{owner}");
            let response = call_api_json(config, Method::GET, &path, None)?;
            print_json("defi_position", response)
        }
        "stats" => {
            let response = call_api_json(config, Method::GET, "/defi/stats", None)?;
            print_json("defi_stats", response)
        }
        other => Err(AppError::Command(format!("未知 defi 子命令: {other}"))),
    }
}

/// 处理 NFT 相关命令（通过 API 调用）。
fn handle_nft_command(config: &AppConfig, args: &[String]) -> AppResult<()> {
    if args.len() < 2 {
        return Err(AppError::Command(
            "nft 命令缺少子命令，可用: mint/list/cancel/buy/token/listing/active-listings/owner-tokens"
                .to_string(),
        ));
    }

    match args[1].as_str() {
        "mint" => {
            let owner = require_arg(args, 2, "owner")?;
            let name = require_arg(args, 3, "name")?;
            let description = require_arg(args, 4, "description")?;
            let image_url = require_arg(args, 5, "image_url")?;
            let response = call_api_json(
                config,
                Method::POST,
                "/nft/mint",
                Some(json!({
                    "owner": owner,
                    "name": name,
                    "description": description,
                    "image_url": image_url
                })),
            )?;
            print_json("nft_mint", response)
        }
        "list" => {
            let seller = require_arg(args, 2, "seller")?;
            let token_id = require_arg(args, 3, "token_id")?;
            let price = parse_u64_arg(args, 4, "price")?;
            let response = call_api_json(
                config,
                Method::POST,
                "/nft/list",
                Some(json!({
                    "seller": seller,
                    "token_id": token_id,
                    "price": price
                })),
            )?;
            print_json("nft_list", response)
        }
        "cancel" => {
            let seller = require_arg(args, 2, "seller")?;
            let listing_id = require_arg(args, 3, "listing_id")?;
            let response = call_api_json(
                config,
                Method::POST,
                "/nft/cancel",
                Some(json!({
                    "seller": seller,
                    "listing_id": listing_id
                })),
            )?;
            print_json("nft_cancel", response)
        }
        "buy" => {
            let buyer = require_arg(args, 2, "buyer")?;
            let listing_id = require_arg(args, 3, "listing_id")?;
            let response = call_api_json(
                config,
                Method::POST,
                "/nft/buy",
                Some(json!({
                    "buyer": buyer,
                    "listing_id": listing_id
                })),
            )?;
            print_json("nft_buy", response)
        }
        "token" => {
            let token_id = require_arg(args, 2, "token_id")?;
            let path = format!("/nft/token/{token_id}");
            let response = call_api_json(config, Method::GET, &path, None)?;
            print_json("nft_token", response)
        }
        "listing" => {
            let listing_id = require_arg(args, 2, "listing_id")?;
            let path = format!("/nft/listing/{listing_id}");
            let response = call_api_json(config, Method::GET, &path, None)?;
            print_json("nft_listing", response)
        }
        "active-listings" => {
            let response = call_api_json(config, Method::GET, "/nft/listings/active", None)?;
            print_json("nft_active_listings", response)
        }
        "owner-tokens" => {
            let owner = require_arg(args, 2, "owner")?;
            let path = format!("/nft/owner/{owner}/tokens");
            let response = call_api_json(config, Method::GET, &path, None)?;
            print_json("nft_owner_tokens", response)
        }
        other => Err(AppError::Command(format!("未知 nft 子命令: {other}"))),
    }
}

/// 打印 JSON 结果。
fn print_json(label: &str, value: Value) -> AppResult<()> {
    let text = serde_json::to_string_pretty(&value)
        .map_err(|error| AppError::Command(format!("JSON 序列化失败: {error}")))?;
    println!("{label}:");
    println!("{text}");
    Ok(())
}

/// 调用 API 并返回 JSON 结果。
fn call_api_json(
    config: &AppConfig,
    method: Method,
    path: &str,
    payload: Option<Value>,
) -> AppResult<Value> {
    let client = Client::new();
    let url = format!("{}{}", api_base_url(config), path);
    let mut request = client.request(method, &url);
    if let Some(body) = payload {
        request = request.json(&body);
    }

    let response = request
        .send()
        .map_err(|error| AppError::Command(format!("调用 API 失败: {error}")))?;
    let status = response.status();
    let body_text = response
        .text()
        .map_err(|error| AppError::Command(format!("读取 API 响应失败: {error}")))?;
    let body_json: Value = serde_json::from_str(&body_text).unwrap_or_else(|_| {
        json!({
            "ok": status.is_success(),
            "raw_body": body_text
        })
    });

    if status.is_success() {
        Ok(body_json)
    } else {
        let message = body_json
            .get("error")
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| body_json.to_string());
        Err(AppError::Command(format!(
            "API 返回错误({status}): {message}"
        )))
    }
}

/// 获取 API 根地址。
fn api_base_url(config: &AppConfig) -> String {
    format!("http://{}", config.api_listen_addr())
}

/// 读取必填参数。
fn require_arg<'a>(args: &'a [String], index: usize, name: &str) -> AppResult<&'a str> {
    args.get(index)
        .map(String::as_str)
        .filter(|value| !value.trim().is_empty())
        .ok_or_else(|| AppError::Command(format!("缺少参数: {name}")))
}

/// 读取并解析 u64 参数。
fn parse_u64_arg(args: &[String], index: usize, name: &str) -> AppResult<u64> {
    let raw = require_arg(args, index, name)?;
    raw.parse::<u64>()
        .map_err(|error| AppError::Command(format!("{name} 参数解析失败: {error}")))
}

/// 构造并签名一笔转账交易，用于提交到链接口。
fn build_signed_transfer_tx(
    from: &str,
    to: &str,
    amount: u64,
    private_key: &str,
    public_key: &str,
    payload: Option<Vec<u8>>,
) -> AppResult<Transaction> {
    if amount == 0 {
        return Err(AppError::Command("amount 必须大于 0".to_string()));
    }

    let mut tx = Transaction::new(from.to_string(), to.to_string(), amount, payload);
    tx.sign_with_private_key(private_key, public_key)
        .map_err(|error| AppError::Command(format!("交易签名失败: {error}")))?;
    tx.validate_for_chain()
        .map_err(|error| AppError::Command(format!("交易校验失败: {error}")))?;
    Ok(tx)
}

/// 构造并签名一笔合约调用交易，用于提交到链交易接口。
fn build_signed_contract_call_tx(
    from: &str,
    to: &str,
    amount: u64,
    nonce: u64,
    private_key: &str,
    public_key: &str,
    payload: Vec<u8>,
) -> AppResult<Transaction> {
    if amount == 0 {
        return Err(AppError::Command("amount 必须大于 0".to_string()));
    }
    if payload.is_empty() {
        return Err(AppError::Command("payload 不能为空".to_string()));
    }

    let mut tx = Transaction::new_with_kind(
        TransactionKind::ContractCall,
        from.to_string(),
        to.to_string(),
        amount,
        nonce,
        Some(payload),
    );
    tx.sign_with_private_key(private_key, public_key)
        .map_err(|error| AppError::Command(format!("交易签名失败: {error}")))?;
    tx.validate_for_chain()
        .map_err(|error| AppError::Command(format!("交易校验失败: {error}")))?;
    Ok(tx)
}

/// 从文件读取合约源码文本。
fn read_contract_source(path: &str) -> AppResult<String> {
    let source = fs::read_to_string(path)
        .map_err(|error| AppError::Command(format!("读取合约文件失败: {error}")))?;
    if source.trim().is_empty() {
        return Err(AppError::Command("合约文件内容不能为空".to_string()));
    }
    Ok(source)
}

/// 打印帮助信息。
fn print_help() {
    println!("RustChain Lab CLI");
    println!("用法:");
    println!("  rustchain-cli wallet create <password>");
    println!("  rustchain-cli tx sign-demo [amount]");
    println!("  rustchain-cli chain info");
    println!("  rustchain-cli chain balance <address>");
    println!("  rustchain-cli chain contract-state <address>");
    println!("  rustchain-cli chain contract-events <address>");
    println!("  rustchain-cli chain contract-field <address> <field>");
    println!("  rustchain-cli chain mine <miner_address>");
    println!(
        "  rustchain-cli chain transfer <from> <to> <amount> <private_key> <public_key> [payload]"
    );
    println!(
        "  rustchain-cli chain contract-call-file <from> <to> <amount> <private_key> <public_key> <source_path> [nonce]"
    );
    println!("  rustchain-cli chain history-block <block_hash>");
    println!("  rustchain-cli chain history-tx <tx_id>");
    println!("  rustchain-cli p2p status");
    println!("  rustchain-cli p2p peers");
    println!("  rustchain-cli p2p register-peer <peer_id> <address>");
    println!("  rustchain-cli p2p ping <peer_id> <address> <sequence> [nonce]");
    println!("  rustchain-cli p2p get-chain-status <peer_id> <address> <sequence>");
    println!(
        "  rustchain-cli p2p chain-status <peer_id> <address> <sequence> <best_height> <best_hash> [difficulty]"
    );
    println!("  rustchain-cli p2p get-blocks <peer_id> <address> <sequence> <from_height> [limit]");
    println!("  rustchain-cli vm compile-file <source_path>");
    println!("  rustchain-cli vm execute-file <source_path> [max_steps]");
    println!("  rustchain-cli defi deposit <owner> <amount>");
    println!("  rustchain-cli defi borrow <owner> <amount>");
    println!("  rustchain-cli defi repay <owner> <amount>");
    println!("  rustchain-cli defi withdraw <owner> <amount>");
    println!("  rustchain-cli defi liquidate <borrower> <amount>");
    println!("  rustchain-cli defi position <owner>");
    println!("  rustchain-cli defi stats");
    println!("  rustchain-cli nft mint <owner> <name> <description> <image_url>");
    println!("  rustchain-cli nft list <seller> <token_id> <price>");
    println!("  rustchain-cli nft cancel <seller> <listing_id>");
    println!("  rustchain-cli nft buy <buyer> <listing_id>");
    println!("  rustchain-cli nft token <token_id>");
    println!("  rustchain-cli nft listing <listing_id>");
    println!("  rustchain-cli nft active-listings");
    println!("  rustchain-cli nft owner-tokens <owner>");
    println!("  rustchain-cli help");
}

#[cfg(test)]
mod tests {
    use super::{
        api_base_url, build_signed_contract_call_tx, build_signed_transfer_tx,
        read_contract_source, require_arg,
    };
    use rustchain_common::AppConfig;
    use rustchain_crypto::wallet::create_wallet;
    use std::{fs, path::PathBuf};

    /// 验证 API 根地址组装正确。
    #[test]
    fn api_base_url_should_be_http_host_port() {
        let config = AppConfig {
            app_name: "rustchain-cli".to_string(),
            log_level: "info".to_string(),
            api_host: "127.0.0.1".to_string(),
            api_port: 8088,
            p2p_bind_addr: "0.0.0.0:7000".to_string(),
            seed_nodes: Vec::new(),
            data_dir: "./data".to_string(),
            mining_difficulty: 2,
            mining_reward: 50,
        };

        assert_eq!(api_base_url(&config), "http://127.0.0.1:8088");
    }

    /// 验证可以构造并签名合法转账交易。
    #[test]
    fn build_signed_transfer_tx_should_work() {
        let (sender_wallet, sender_key_pair) =
            create_wallet("sender-pass").expect("钱包创建应成功");
        let (receiver_wallet, _) = create_wallet("receiver-pass").expect("钱包创建应成功");

        let tx = build_signed_transfer_tx(
            &sender_wallet.address,
            &receiver_wallet.address,
            10,
            &sender_key_pair.private_key,
            &sender_key_pair.public_key,
            Some(b"unit-test".to_vec()),
        )
        .expect("交易构造应成功");

        assert_eq!(tx.from, sender_wallet.address);
        assert_eq!(tx.to, receiver_wallet.address);
        assert_eq!(tx.amount, 10);
        assert!(tx.signature.is_some());
    }

    /// 验证金额为 0 的转账会被拒绝。
    #[test]
    fn build_signed_transfer_tx_with_zero_amount_should_fail() {
        let (sender_wallet, sender_key_pair) =
            create_wallet("sender-pass").expect("钱包创建应成功");
        let (receiver_wallet, _) = create_wallet("receiver-pass").expect("钱包创建应成功");

        let result = build_signed_transfer_tx(
            &sender_wallet.address,
            &receiver_wallet.address,
            0,
            &sender_key_pair.private_key,
            &sender_key_pair.public_key,
            None,
        );

        assert!(result.is_err());
    }

    /// 验证可以构造并签名合约调用交易。
    #[test]
    fn build_signed_contract_call_tx_should_work() {
        let (sender_wallet, sender_key_pair) =
            create_wallet("sender-pass").expect("钱包创建应成功");

        let tx = build_signed_contract_call_tx(
            &sender_wallet.address,
            "contract-demo",
            1,
            9,
            &sender_key_pair.private_key,
            &sender_key_pair.public_key,
            b"LOAD_CONST 1\nHALT\n".to_vec(),
        )
        .expect("合约调用交易构造应成功");

        assert_eq!(
            tx.kind,
            rustchain_core::transaction::TransactionKind::ContractCall
        );
        assert_eq!(tx.nonce, 9);
        assert!(tx.signature.is_some());
    }

    /// 验证命令参数读取逻辑支持历史查询场景。
    #[test]
    fn require_arg_should_read_history_query_identifiers() {
        let args = vec![
            "chain".to_string(),
            "history-block".to_string(),
            "block-hash-001".to_string(),
        ];

        let block_hash = require_arg(&args, 2, "block_hash").expect("应读取 block_hash");
        assert_eq!(block_hash, "block-hash-001");
    }

    /// 验证可以从文件读取合约源码。
    #[test]
    fn read_contract_source_should_work() {
        let path = temp_contract_file_path("vm-source-read");
        fs::write(&path, "LOAD_CONST 1\nHALT\n").expect("写入临时文件应成功");

        let source =
            read_contract_source(path.to_str().expect("路径应为 UTF-8")).expect("读取源码应成功");
        assert!(source.contains("LOAD_CONST 1"));

        let _ = fs::remove_file(path);
    }

    /// 生成测试临时文件路径。
    fn temp_contract_file_path(prefix: &str) -> PathBuf {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应可用")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{ts}.vm"))
    }
}
