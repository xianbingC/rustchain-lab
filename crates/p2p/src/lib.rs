//! P2P messaging and synchronization primitives.

pub mod codec;
pub mod error;
pub mod message;
pub mod peer;
pub mod queue;

pub use error::{P2pError, P2pResult};
