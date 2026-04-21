use crate::{
    codec::MessageCodec,
    error::{P2pError, P2pResult},
    message::{ChainStatus, NetworkMessage},
    peer::PeerRegistry,
    queue::{OrderedMessageQueue, SequencedMessage},
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 待发送消息信封，交给传输层处理。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundEnvelope {
    /// 目标节点 ID。
    pub target_peer_id: String,
    /// 消息内容。
    pub message: NetworkMessage,
}

/// 单次处理报告，便于上层统计。
#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
pub struct ProcessReport {
    /// 本次处理的消息数量。
    pub processed: usize,
    /// 本次产生的待发送消息。
    pub outbound: Vec<OutboundEnvelope>,
}

/// P2P 同步引擎，负责顺序处理消息并生成响应动作。
pub struct SyncEngine {
    local_peer_id: String,
    local_chain_status: ChainStatus,
    peers: PeerRegistry,
    queues: HashMap<String, OrderedMessageQueue>,
}

impl SyncEngine {
    /// 创建同步引擎。
    pub fn new(local_peer_id: impl Into<String>, local_chain_status: ChainStatus) -> Self {
        Self {
            local_peer_id: local_peer_id.into(),
            local_chain_status,
            peers: PeerRegistry::new(),
            queues: HashMap::new(),
        }
    }

    /// 返回本地节点 ID。
    pub fn local_peer_id(&self) -> &str {
        &self.local_peer_id
    }

    /// 返回本地链状态。
    pub fn local_chain_status(&self) -> &ChainStatus {
        &self.local_chain_status
    }

    /// 更新本地链状态。
    pub fn update_local_chain_status(&mut self, status: ChainStatus) {
        self.local_chain_status = status;
    }

    /// 注册已知节点。
    pub fn register_peer(&mut self, peer_id: impl Into<String>, address: impl Into<String>) {
        let peer_id = peer_id.into();
        self.peers.upsert(peer_id.clone(), address);
        self.queues
            .entry(peer_id)
            .or_insert_with(|| OrderedMessageQueue::new(1));
    }

    /// 处理编码后的入站消息。
    pub fn on_incoming_encoded(
        &mut self,
        peer_id: impl Into<String>,
        address: impl Into<String>,
        sequence: u64,
        payload: &[u8],
    ) -> P2pResult<ProcessReport> {
        let message = MessageCodec::decode(payload)?;
        self.on_incoming_message(peer_id, address, sequence, message)
    }

    /// 处理入站消息并按序执行。
    pub fn on_incoming_message(
        &mut self,
        peer_id: impl Into<String>,
        address: impl Into<String>,
        sequence: u64,
        message: NetworkMessage,
    ) -> P2pResult<ProcessReport> {
        message
            .validate_basic()
            .map_err(|error| P2pError::InvalidMessage(error.to_string()))?;

        let peer_id = peer_id.into();
        self.peers.upsert(peer_id.clone(), address);

        let queue = self
            .queues
            .entry(peer_id.clone())
            .or_insert_with(|| OrderedMessageQueue::new(1));

        queue.push(SequencedMessage::new(peer_id.clone(), sequence, message))?;
        let ready = queue.pop_all_ready();

        let mut report = ProcessReport::default();
        for item in ready {
            report.processed = report.processed.saturating_add(1);
            let outbound = self.handle_message(&peer_id, item.message)?;
            report.outbound.extend(outbound);
        }

        Ok(report)
    }

    /// 对外发消息编码。
    pub fn encode_outbound(&self, message: &NetworkMessage) -> P2pResult<Vec<u8>> {
        MessageCodec::encode(message)
    }

    /// 获取只读节点注册表。
    pub fn peers(&self) -> &PeerRegistry {
        &self.peers
    }

    /// 返回全部节点快照。
    pub fn peer_snapshot(&self) -> Vec<crate::peer::PeerInfo> {
        self.peers.snapshot()
    }

    /// 返回当前已知节点数。
    pub fn peer_count(&self) -> usize {
        self.peers.len()
    }

    /// 获取某节点的下一期望序号。
    pub fn next_expected_sequence(&self, peer_id: &str) -> Option<u64> {
        self.queues
            .get(peer_id)
            .map(OrderedMessageQueue::next_expected)
    }

    /// 广播消息到当前已连接节点。
    pub fn broadcast_to_connected(&self, message: NetworkMessage) -> Vec<OutboundEnvelope> {
        self.peers
            .connected_peers()
            .into_iter()
            .map(|peer| OutboundEnvelope {
                target_peer_id: peer.id.clone(),
                message: message.clone(),
            })
            .collect()
    }

    /// 处理单条已就绪消息。
    fn handle_message(
        &mut self,
        peer_id: &str,
        message: NetworkMessage,
    ) -> P2pResult<Vec<OutboundEnvelope>> {
        if let Some(peer) = self.peers.get_mut(peer_id) {
            peer.mark_alive(None);
        }

        let mut outbound = Vec::new();
        match message {
            NetworkMessage::Ping { nonce, timestamp } => {
                outbound.push(OutboundEnvelope {
                    target_peer_id: peer_id.to_string(),
                    message: NetworkMessage::Pong { nonce, timestamp },
                });
            }
            NetworkMessage::Handshake(handshake) => {
                if handshake.node_id.trim().is_empty() {
                    return Err(P2pError::InvalidArgument(
                        "handshake.node_id 不能为空".to_string(),
                    ));
                }

                self.peers
                    .upsert(handshake.node_id.clone(), handshake.listen_addr.clone());
                if let Some(peer) = self.peers.get_mut(&handshake.node_id) {
                    peer.update_chain_tip(handshake.best_height, handshake.best_hash);
                    peer.mark_alive(None);
                }

                outbound.push(OutboundEnvelope {
                    target_peer_id: peer_id.to_string(),
                    message: NetworkMessage::ChainStatus(self.local_chain_status.clone()),
                });
            }
            NetworkMessage::GetChainStatus => {
                outbound.push(OutboundEnvelope {
                    target_peer_id: peer_id.to_string(),
                    message: NetworkMessage::ChainStatus(self.local_chain_status.clone()),
                });
            }
            NetworkMessage::ChainStatus(status) => {
                if let Some(peer) = self.peers.get_mut(peer_id) {
                    peer.update_chain_tip(status.best_height, status.best_hash.clone());
                }

                if status.best_height > self.local_chain_status.best_height {
                    outbound.push(OutboundEnvelope {
                        target_peer_id: peer_id.to_string(),
                        message: NetworkMessage::GetBlocks {
                            from_height: self.local_chain_status.best_height.saturating_add(1),
                            limit: 128,
                        },
                    });
                }
            }
            NetworkMessage::GetMempool => {
                outbound.push(OutboundEnvelope {
                    target_peer_id: peer_id.to_string(),
                    message: NetworkMessage::Mempool {
                        transactions: Vec::new(),
                    },
                });
            }
            NetworkMessage::GetBlocks { .. } => {
                // 当前阶段尚未接入区块数据库，先返回空结果，保证协议通路可用。
                outbound.push(OutboundEnvelope {
                    target_peer_id: peer_id.to_string(),
                    message: NetworkMessage::Blocks { blocks: Vec::new() },
                });
            }
            NetworkMessage::Pong { .. }
            | NetworkMessage::NewTransaction { .. }
            | NetworkMessage::NewBlock { .. }
            | NetworkMessage::Blocks { .. }
            | NetworkMessage::Mempool { .. } => {}
        }

        Ok(outbound)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::Handshake;

    fn local_status(height: u64) -> ChainStatus {
        ChainStatus {
            chain_id: "rustchain-lab-dev".to_string(),
            best_height: height,
            best_hash: format!("0x{height}"),
            difficulty: 2,
            genesis_hash: "0xgenesis".to_string(),
        }
    }

    /// 验证 Ping 能触发 Pong 响应。
    #[test]
    fn ping_should_produce_pong() {
        let mut engine = SyncEngine::new("local-node", local_status(5));
        let payload = MessageCodec::encode(&NetworkMessage::Ping {
            nonce: 7,
            timestamp: 123,
        })
        .expect("编码应当成功");

        let report = engine
            .on_incoming_encoded("peer-a", "/ip4/127.0.0.1/tcp/7001", 1, &payload)
            .expect("处理应当成功");

        assert_eq!(report.processed, 1);
        assert_eq!(report.outbound.len(), 1);
        assert_eq!(
            report.outbound[0].message,
            NetworkMessage::Pong {
                nonce: 7,
                timestamp: 123
            }
        );
    }

    /// 验证乱序输入会按序处理并保持响应顺序。
    #[test]
    fn out_of_order_messages_should_be_processed_in_sequence() {
        let mut engine = SyncEngine::new("local-node", local_status(3));
        let msg2 = NetworkMessage::Ping {
            nonce: 9,
            timestamp: 10,
        };
        let msg1 = NetworkMessage::GetChainStatus;

        let report2 = engine
            .on_incoming_message("peer-a", "/ip4/127.0.0.1/tcp/7001", 2, msg2)
            .expect("处理应当成功");
        assert_eq!(report2.processed, 0);

        let report1 = engine
            .on_incoming_message("peer-a", "/ip4/127.0.0.1/tcp/7001", 1, msg1)
            .expect("处理应当成功");
        assert_eq!(report1.processed, 2);
        assert_eq!(report1.outbound.len(), 2);
        assert!(matches!(
            report1.outbound[0].message,
            NetworkMessage::ChainStatus(_)
        ));
        assert!(matches!(
            report1.outbound[1].message,
            NetworkMessage::Pong { .. }
        ));
    }

    /// 验证远端高度更高时会触发补块请求。
    #[test]
    fn higher_remote_chain_should_trigger_get_blocks() {
        let mut engine = SyncEngine::new("local-node", local_status(2));
        let handshake = NetworkMessage::Handshake(Handshake {
            node_id: "peer-a".to_string(),
            protocol_version: "1.0.0".to_string(),
            listen_addr: "/ip4/127.0.0.1/tcp/7001".to_string(),
            best_height: 2,
            best_hash: "0x2".to_string(),
        });
        engine
            .on_incoming_message("peer-a", "/ip4/127.0.0.1/tcp/7001", 1, handshake)
            .expect("握手应当成功");

        let report = engine
            .on_incoming_message(
                "peer-a",
                "/ip4/127.0.0.1/tcp/7001",
                2,
                NetworkMessage::ChainStatus(local_status(8)),
            )
            .expect("状态同步应当成功");

        assert_eq!(report.processed, 1);
        assert_eq!(report.outbound.len(), 1);
        assert_eq!(
            report.outbound[0].message,
            NetworkMessage::GetBlocks {
                from_height: 3,
                limit: 128,
            }
        );
    }

    /// 验证广播仅面向已连接节点。
    #[test]
    fn broadcast_should_target_connected_peers() {
        let mut engine = SyncEngine::new("local-node", local_status(2));
        engine.register_peer("peer-a", "/ip4/127.0.0.1/tcp/7001");
        engine.register_peer("peer-b", "/ip4/127.0.0.1/tcp/7002");

        let _ = engine
            .on_incoming_message(
                "peer-a",
                "/ip4/127.0.0.1/tcp/7001",
                1,
                NetworkMessage::GetChainStatus,
            )
            .expect("处理应当成功");

        let outbound = engine.broadcast_to_connected(NetworkMessage::GetMempool);
        assert_eq!(outbound.len(), 1);
        assert_eq!(outbound[0].target_peer_id, "peer-a");
    }
}
