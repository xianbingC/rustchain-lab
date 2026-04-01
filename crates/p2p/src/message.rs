use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize)]
pub enum NetworkMessage {
    Ping,
    NewTransaction(Vec<u8>),
    NewBlock(Vec<u8>),
    SyncRequest { from_height: u64 },
    SyncResponse { blocks: Vec<Vec<u8>> },
}
