use crate::{
    error::{AppError, AppResult},
};
use serde::{Deserialize, Serialize};
use std::env;

/// 项目公共配置对象。
#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct AppConfig {
    /// 服务名称，用于日志和诊断输出。
    pub app_name: String,
    /// 日志级别。
    pub log_level: String,
    /// REST API 绑定主机。
    pub api_host: String,
    /// REST API 端口。
    pub api_port: u16,
    /// P2P 节点绑定地址。
    pub p2p_bind_addr: String,
    /// 初始种子节点列表，使用逗号分隔。
    pub seed_nodes: Vec<String>,
    /// 链数据目录。
    pub data_dir: String,
    /// 挖矿难度。
    pub mining_difficulty: u32,
    /// 出块奖励。
    pub mining_reward: u64,
}

impl AppConfig {
    /// 从环境变量加载配置，缺失时使用项目默认值。
    pub fn from_env(app_name: impl Into<String>) -> AppResult<Self> {
        let app_name = app_name.into();

        Ok(Self {
            app_name,
            log_level: env_or_default("RUSTCHAIN_LOG_LEVEL", "info"),
            api_host: env_or_default("RUSTCHAIN_API_HOST", "127.0.0.1"),
            api_port: parse_env_or_default("RUSTCHAIN_API_PORT", 8080)?,
            p2p_bind_addr: env_or_default("RUSTCHAIN_P2P_BIND_ADDR", "0.0.0.0:7000"),
            seed_nodes: parse_seed_nodes("RUSTCHAIN_SEED_NODES"),
            data_dir: env_or_default("RUSTCHAIN_DATA_DIR", "./data"),
            mining_difficulty: parse_env_or_default("RUSTCHAIN_MINING_DIFFICULTY", 2)?,
            mining_reward: parse_env_or_default("RUSTCHAIN_MINING_REWARD", 50)?,
        })
    }

    /// 生成 REST API 的监听地址。
    pub fn api_listen_addr(&self) -> String {
        format!("{}:{}", self.api_host, self.api_port)
    }
}

/// 读取字符串环境变量，不存在时回退到默认值。
fn env_or_default(key: &str, default: &str) -> String {
    env::var(key).unwrap_or_else(|_| default.to_string())
}

/// 读取并解析环境变量，不存在时回退到默认值。
fn parse_env_or_default<T>(key: &str, default: T) -> AppResult<T>
where
    T: std::str::FromStr + Copy,
    T::Err: std::fmt::Display,
{
    match env::var(key) {
        Ok(value) => value.parse::<T>().map_err(|error| {
            AppError::Config(format!("{key} 解析失败: {error}"))
        }),
        Err(_) => Ok(default),
    }
}

/// 解析逗号分隔的种子节点列表。
fn parse_seed_nodes(key: &str) -> Vec<String> {
    env::var(key)
        .unwrap_or_default()
        .split(',')
        .map(str::trim)
        .filter(|item| !item.is_empty())
        .map(ToOwned::to_owned)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证默认配置可以正确加载。
    #[test]
    fn should_load_default_config() {
        let config = AppConfig::from_env("test-app").expect("默认配置应当可用");

        assert_eq!(config.app_name, "test-app");
        assert_eq!(config.api_host, "127.0.0.1");
        assert_eq!(config.api_port, 8080);
        assert_eq!(config.mining_difficulty, 2);
        assert_eq!(config.mining_reward, 50);
    }
}
