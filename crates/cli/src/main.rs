use rustchain_common::{logging::init_logging, AppConfig, AppResult};

/// CLI 程序入口。
fn main() {
    if let Err(error) = run() {
        eprintln!("CLI 启动失败: {error}");
        std::process::exit(1);
    }
}

/// 执行 CLI 初始化流程。
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

    println!("RustChain Lab CLI 已启动");
    println!("当前数据目录: {}", config.data_dir);
    println!("默认挖矿难度: {}", config.mining_difficulty);

    Ok(())
}
