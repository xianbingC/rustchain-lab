use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::BTreeMap,
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

/// Kademlia 风格最近邻查询结果。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PeerDistance {
    /// 节点信息。
    pub peer: PeerInfo,
    /// 目标 ID 与节点 ID 的异或距离（十六进制）。
    pub xor_distance_hex: String,
}

/// DHT 桶视图项。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DhtBucket {
    /// 桶索引（数值越大表示距离越近）。
    pub bucket_index: u8,
    /// 当前桶包含的节点 ID 列表。
    pub peer_ids: Vec<String>,
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

    /// 获取全部节点快照（克隆），便于上层做只读展示。
    pub fn snapshot(&self) -> Vec<PeerInfo> {
        self.peers.values().cloned().collect()
    }

    /// 按目标节点 ID 返回距离最近的节点列表（Kademlia 异或距离）。
    pub fn nearest_peers(&self, target_id: &str, limit: usize) -> Vec<PeerDistance> {
        if target_id.trim().is_empty() || limit == 0 {
            return Vec::new();
        }

        let target_digest = digest_node_id(target_id);
        let mut pairs = self
            .peers
            .values()
            .cloned()
            .map(|peer| {
                let distance_bytes = xor_distance_bytes(&target_digest, &digest_node_id(&peer.id));
                let peer_id = peer.id.clone();
                (
                    distance_bytes,
                    peer_id,
                    PeerDistance {
                        peer,
                        xor_distance_hex: hex::encode(distance_bytes),
                    },
                )
            })
            .collect::<Vec<_>>();

        // 距离相同按节点 ID 稳定排序，避免返回顺序抖动。
        pairs.sort_by(|left, right| left.0.cmp(&right.0).then_with(|| left.1.cmp(&right.1)));
        pairs
            .into_iter()
            .take(limit)
            .map(|(_, _, distance)| distance)
            .collect()
    }

    /// 按目标节点 ID 生成 DHT 桶视图（基于 XOR 距离前缀）。
    pub fn dht_buckets(&self, target_id: &str, bucket_count: u8) -> Vec<DhtBucket> {
        if target_id.trim().is_empty() || bucket_count == 0 {
            return Vec::new();
        }

        let target_digest = digest_node_id(target_id);
        let mut grouped = BTreeMap::<u8, Vec<String>>::new();

        for peer in self.peers.values() {
            let distance = xor_distance_bytes(&target_digest, &digest_node_id(&peer.id));
            let leading_zero_bits = leading_zero_bits(&distance);
            // 将 0..=256 的前导零位映射到桶索引，桶索引越大表示越接近目标。
            let bucket_index = leading_zero_bits.min(bucket_count as usize - 1) as u8;
            grouped
                .entry(bucket_index)
                .or_default()
                .push(peer.id.clone());
        }

        let mut buckets = grouped
            .into_iter()
            .map(|(bucket_index, mut peer_ids)| {
                peer_ids.sort();
                DhtBucket {
                    bucket_index,
                    peer_ids,
                }
            })
            .collect::<Vec<_>>();
        // 输出按距离从近到远排序，便于运维排查。
        buckets.sort_by(|left, right| right.bucket_index.cmp(&left.bucket_index));
        buckets
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

/// 计算节点 ID 的 SHA-256 摘要，用作 Kademlia 路由键。
fn digest_node_id(node_id: &str) -> [u8; 32] {
    let digest = Sha256::digest(node_id.as_bytes());
    let mut output = [0u8; 32];
    output.copy_from_slice(&digest);
    output
}

/// 计算两段摘要的异或距离。
fn xor_distance_bytes(left: &[u8; 32], right: &[u8; 32]) -> [u8; 32] {
    let mut out = [0u8; 32];
    for (index, slot) in out.iter_mut().enumerate() {
        *slot = left[index] ^ right[index];
    }
    out
}

/// 计算 256 位字节数组的前导零位数。
fn leading_zero_bits(raw: &[u8; 32]) -> usize {
    let mut count = 0usize;
    for byte in raw {
        if *byte == 0 {
            count += 8;
        } else {
            count += byte.leading_zeros() as usize;
            break;
        }
    }
    count
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

    /// 验证节点快照可以返回全部节点。
    #[test]
    fn snapshot_should_include_all_peers() {
        let mut registry = PeerRegistry::new();
        registry.upsert("node-a", "/ip4/127.0.0.1/tcp/7001");
        registry.upsert("node-b", "/ip4/127.0.0.1/tcp/7002");

        let snapshot = registry.snapshot();
        assert_eq!(snapshot.len(), 2);
    }

    /// 验证最近邻查询会按距离排序并应用数量上限。
    #[test]
    fn nearest_peers_should_sort_and_limit() {
        let mut registry = PeerRegistry::new();
        registry.upsert("node-a", "/ip4/127.0.0.1/tcp/7001");
        registry.upsert("node-b", "/ip4/127.0.0.1/tcp/7002");
        registry.upsert("node-c", "/ip4/127.0.0.1/tcp/7003");

        let nearest = registry.nearest_peers("target-node", 2);
        assert_eq!(nearest.len(), 2);
        assert!(nearest[0].xor_distance_hex <= nearest[1].xor_distance_hex);
    }

    /// 验证空目标或零上限会返回空结果。
    #[test]
    fn nearest_peers_with_invalid_input_should_return_empty() {
        let mut registry = PeerRegistry::new();
        registry.upsert("node-a", "/ip4/127.0.0.1/tcp/7001");

        assert!(registry.nearest_peers("", 5).is_empty());
        assert!(registry.nearest_peers("target-node", 0).is_empty());
    }

    /// 验证 DHT 桶视图会按桶索引从近到远排序。
    #[test]
    fn dht_buckets_should_sort_by_bucket_index_desc() {
        let mut registry = PeerRegistry::new();
        registry.upsert("node-a", "/ip4/127.0.0.1/tcp/7001");
        registry.upsert("node-b", "/ip4/127.0.0.1/tcp/7002");
        registry.upsert("node-c", "/ip4/127.0.0.1/tcp/7003");

        let buckets = registry.dht_buckets("target-node", 8);
        assert!(!buckets.is_empty());
        for pair in buckets.windows(2) {
            assert!(pair[0].bucket_index >= pair[1].bucket_index);
        }
    }
}
