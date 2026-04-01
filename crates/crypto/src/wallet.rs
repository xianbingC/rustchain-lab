use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Wallet {
    pub address: String,
    pub public_key: String,
    pub encrypted_private_key: String,
}
