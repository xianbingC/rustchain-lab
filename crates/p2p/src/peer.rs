use serde::{Deserialize, Serialize};
use std::{
    collections::HashMap,
    time::{SystemTime, UNIX_EPOCH},
};

/// 对等节点连接状态。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum PeerStatus {
    /// 尚未完成握手。
    Connecting,
    /// 已连接并可交换消息。
    Connected,
    /// 暂时断开，可重试连接。
    Disconnected,
}

/// 节点元信息。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerInfo {
    /// 节点 ID。
    pub id: String,
    /// 节点网络地址。
    pub address: String,
    /// 当前已知区块高度。
    pub best_height: u64,
    /// 当前已知区块哈希。
    pub best_hash: String,
    /// 连接状态。
    pub status: PeerStatus,
    /// 最后一次心跳时间戳（秒）。
    pub last_seen_secs: u64,
    /// 最近测得的往返延迟（毫秒）。
    pub latency_ms: Option<u64>,
}

impl PeerInfo {
    /// 创建默认节点记录。
    pub fn new(id: impl Into<String>, address: impl Into<String>) -> Self {
        Self {
            id: id.into(),
            address: address.into(),
            best_height: 0,
            best_hash: String::new(),
            status: PeerStatus::Connecting,
            last_seen_secs: now_secs(),
            latency_ms: None,
        }
    }

    /// 更新链状态摘要。
    pub fn update_chain_tip(&mut self, best_height: u64, best_hash: impl Into<String>) {
        self.best_height = best_height;
        self.best_hash = best_hash.into();
        self.last_seen_secs = now_secs();
    }

    /// 更新心跳状态。
    pub fn mark_alive(&mut self, latency_ms: Option<u64>) {
        self.status = PeerStatus::Connected;
        self.latency_ms = latency_ms;
        self.last_seen_secs = now_secs();
    }
}

/// 对等节点注册表。
#[derive(Debug, Default)]
pub struct PeerRegistry {
    peers: HashMap<String, PeerInfo>,
}

impl PeerRegistry {
    /// 创建节点注册表。
    pub fn new() -> Self {
        Self::default()
    }

    /// 注册或更新节点基础信息。
    pub fn upsert(&mut self, id: impl Into<String>, address: impl Into<String>) -> &PeerInfo {
        let id = id.into();
        let address = address.into();

        let peer = self
            .peers
            .entry(id.clone())
            .or_insert_with(|| PeerInfo::new(id, address.clone()));
        peer.address = address;
        peer.last_seen_secs = now_secs();
        peer
    }

    /// 按节点 ID 查询。
    pub fn get(&self, id: &str) -> Option<&PeerInfo> {
        self.peers.get(id)
    }

    /// 按节点 ID 可变查询。
    pub fn get_mut(&mut self, id: &str) -> Option<&mut PeerInfo> {
        self.peers.get_mut(id)
    }

    /// 返回当前节点总数。
    pub fn len(&self) -> usize {
        self.peers.len()
    }

    /// 返回节点是否为空。
    pub fn is_empty(&self) -> bool {
        self.peers.is_empty()
    }

    /// 获取连接中的节点列表。
    pub fn connected_peers(&self) -> Vec<&PeerInfo> {
        self.peers
            .values()
            .filter(|peer| peer.status == PeerStatus::Connected)
            .collect()
    }

    /// 标记节点断开。
    pub fn mark_disconnected(&mut self, id: &str) {
        if let Some(peer) = self.peers.get_mut(id) {
            peer.status = PeerStatus::Disconnected;
            peer.last_seen_secs = now_secs();
        }
    }
}

/// 返回当前时间戳（秒）。
fn now_secs() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_secs())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证节点注册与查询流程。
    #[test]
    fn peer_registry_upsert_should_work() {
        let mut registry = PeerRegistry::new();
        registry.upsert("node-a", "/ip4/127.0.0.1/tcp/7001");

        let peer = registry.get("node-a").expect("节点应当存在");
        assert_eq!(peer.id, "node-a");
        assert_eq!(peer.status, PeerStatus::Connecting);
        assert_eq!(registry.len(), 1);
    }

    /// 验证节点状态更新与筛选。
    #[test]
    fn connected_peers_filter_should_work() {
        let mut registry = PeerRegistry::new();
        registry.upsert("node-a", "/ip4/127.0.0.1/tcp/7001");
        registry.upsert("node-b", "/ip4/127.0.0.1/tcp/7002");

        registry
            .get_mut("node-a")
            .expect("节点应当存在")
            .mark_alive(Some(12));
        registry.mark_disconnected("node-b");

        let connected = registry.connected_peers();
        assert_eq!(connected.len(), 1);
        assert_eq!(connected[0].id, "node-a");
    }
}
