//! 公共基础设施模块。

pub mod config;
pub mod error;
pub mod logging;

pub use config::AppConfig;
pub use error::{AppError, AppResult};
