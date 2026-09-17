//! RustChain REST API 服务入口。
//!
//! 模块划分：
//! - `error`：业务错误到 HTTP 状态码的统一映射
//! - `state`：进程内共享状态与锁访问辅助
//! - `chain_ops`：链侧共享操作（持久化、编码、同步区块接收）
//! - `handlers`：按业务域拆分的 HTTP 处理器
//! - `routes`：路由注册表

mod chain_ops;
mod error;
mod handlers;
mod routes;
mod state;

#[cfg(test)]
mod tests;

use rustchain_common::{logging::init_logging, AppConfig, AppResult};
use tokio::net::TcpListener;

/// API 程序入口。
#[tokio::main]
async fn main() {
    if let Err(error) = run().await {
        eprintln!("API 启动失败: {error}");
        std::process::exit(1);
    }
}

/// 执行 API 初始化和监听流程。
async fn run() -> AppResult<()> {
    let config = AppConfig::from_env("rustchain-api")?;
    init_logging(&config)?;

    let app = routes::build_app(state::default_app_state_with_config(&config)?);
    let listen_addr = config.api_listen_addr();
    let listener = TcpListener::bind(&listen_addr).await?;

    tracing::info!(
        app = %config.app_name,
        listen_addr = %listen_addr,
        p2p_bind_addr = %config.p2p_bind_addr,
        difficulty = config.mining_difficulty,
        reward = config.mining_reward,
        target_block_time_secs = config.target_block_time_secs,
        difficulty_adjustment_interval = config.difficulty_adjustment_interval,
        "API 服务启动成功"
    );

    axum::serve(listener, app)
        .await
        .map_err(|error| rustchain_common::AppError::Io(std::io::Error::other(error)))?;

    Ok(())
}
