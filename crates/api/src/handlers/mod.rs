//! HTTP 处理器模块。
//!
//! 按业务域拆分，路由注册集中在 `main.rs::build_app`。

pub(crate) mod chain;
pub(crate) mod defi;
pub(crate) mod health;
pub(crate) mod nft;
pub(crate) mod p2p;
pub(crate) mod vm;
pub(crate) mod wallet;
