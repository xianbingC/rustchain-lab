use axum::{routing::get, Json, Router};
use rustchain_common::{logging::init_logging, AppConfig, AppResult};
use serde_json::json;
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

    let app = Router::new().route("/health", get(health_handler));
    let listen_addr = config.api_listen_addr();
    let listener = TcpListener::bind(&listen_addr).await?;

    tracing::info!(
        app = %config.app_name,
        listen_addr = %listen_addr,
        p2p_bind_addr = %config.p2p_bind_addr,
        "API 服务启动成功"
    );

    axum::serve(listener, app)
        .await
        .map_err(|error| rustchain_common::AppError::Io(std::io::Error::other(error)))?;

    Ok(())
}

/// 健康检查接口。
async fn health_handler() -> Json<serde_json::Value> {
    Json(json!({
        "status": "ok",
        "service": "rustchain-api"
    }))
}
