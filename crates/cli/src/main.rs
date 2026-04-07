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
    dispatch_command(&args)
}

/// 按参数分发命令。
fn dispatch_command(args: &[String]) -> AppResult<()> {
    if args.is_empty() {
        print_help();
        return Ok(());
    }

    match args[0].as_str() {
        "wallet" => handle_wallet_command(args),
        "tx" => handle_tx_command(args),
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
                return Err(AppError::Command(
                    "wallet create 需要密码参数".to_string(),
                ));
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
        other => Err(AppError::Command(format!(
            "未知 wallet 子命令: {other}"
        ))),
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

/// 打印 JSON 结果。
fn print_json(label: &str, value: Value) -> AppResult<()> {
    let text = serde_json::to_string_pretty(&value)
        .map_err(|error| AppError::Command(format!("JSON 序列化失败: {error}")))?;
    println!("{label}:");
    println!("{text}");
    Ok(())
}

/// 打印帮助信息。
fn print_help() {
    println!("RustChain Lab CLI");
    println!("用法:");
    println!("  rustchain-cli wallet create <password>");
    println!("  rustchain-cli tx sign-demo [amount]");
    println!("  rustchain-cli help");
}
