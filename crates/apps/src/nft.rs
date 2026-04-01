use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct NftToken {
    pub token_id: String,
    pub owner: String,
    pub name: String,
    pub description: String,
    pub image_url: String,
}
