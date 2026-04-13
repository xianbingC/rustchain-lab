use reqwest::{blocking::Client, Method};
use rustchain_common::{logging::init_logging, AppConfig, AppError, AppResult};
use rustchain_core::transaction::Transaction;
use rustchain_crypto::wallet::create_wallet;
use serde_json::{json, Value};

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

/// 打印帮助信息。
fn print_help() {
    println!("RustChain Lab CLI");
    println!("用法:");
    println!("  rustchain-cli wallet create <password>");
    println!("  rustchain-cli tx sign-demo [amount]");
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
    use super::api_base_url;
    use rustchain_common::AppConfig;

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
}
