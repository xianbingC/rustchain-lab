use crate::{
    codec::{FrameDecodeBuffer, FramedMessageCodec},
    engine::{OutboundEnvelope, ProcessReport, SyncEngine},
    P2pResult,
};
use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// 已编码的出站传输帧，真实网络层只需要按目标连接写出 bytes。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct OutboundFrame {
    /// 目标节点 ID。
    pub target_peer_id: String,
    /// 长度前缀编码后的完整帧。
    pub bytes: Vec<u8>,
}

/// 单个远端连接的传输会话，负责把字节流转换成顺序消息。
#[derive(Debug, Clone)]
pub struct TransportSession {
    peer_id: String,
    address: String,
    next_sequence: u64,
    decode_buffer: FrameDecodeBuffer,
}

impl TransportSession {
    /// 创建传输会话，序号从 1 开始以复用同步引擎的顺序队列约定。
    pub fn new(peer_id: impl Into<String>, address: impl Into<String>) -> Self {
        Self {
            peer_id: peer_id.into(),
            address: address.into(),
            next_sequence: 1,
            decode_buffer: FrameDecodeBuffer::new(),
        }
    }

    /// 返回下一条完整入站消息将使用的序号。
    pub fn next_sequence(&self) -> u64 {
        self.next_sequence
    }

    /// 返回当前缓存的未完成字节数，用于排查半包堆积问题。
    pub fn buffered_len(&self) -> usize {
        self.decode_buffer.buffered_len()
    }

    /// 追加网络字节并处理所有已完整的消息帧。
    pub fn push_inbound_bytes(
        &mut self,
        engine: &mut SyncEngine,
        bytes: &[u8],
    ) -> P2pResult<ProcessReport> {
        self.decode_buffer.push(bytes);
        let messages = self.decode_buffer.drain_messages()?;

        let mut report = ProcessReport::default();
        for message in messages {
            let item_report = engine.on_incoming_message(
                self.peer_id.clone(),
                self.address.clone(),
                self.next_sequence,
                message,
            )?;
            self.next_sequence = self.next_sequence.saturating_add(1);
            merge_report(&mut report, item_report);
        }

        Ok(report)
    }
}

/// 多连接传输会话池，按远端 peer 维护独立的半包缓存和入站序号。
#[derive(Debug, Clone, Default)]
pub struct TransportSessionPool {
    sessions: HashMap<String, TransportSession>,
}

impl TransportSessionPool {
    /// 创建空会话池。
    pub fn new() -> Self {
        Self::default()
    }

    /// 追加某个 peer 的入站字节；不存在会话时自动创建。
    pub fn push_inbound_bytes(
        &mut self,
        engine: &mut SyncEngine,
        peer_id: impl Into<String>,
        address: impl Into<String>,
        bytes: &[u8],
    ) -> P2pResult<ProcessReport> {
        let peer_id = peer_id.into();
        let address = address.into();
        let session = self
            .sessions
            .entry(peer_id.clone())
            .or_insert_with(|| TransportSession::new(peer_id, address));

        session.push_inbound_bytes(engine, bytes)
    }

    /// 移除断开连接的 peer 会话，返回是否确实存在。
    pub fn remove(&mut self, peer_id: &str) -> bool {
        self.sessions.remove(peer_id).is_some()
    }

    /// 返回当前已维护的传输会话数量。
    pub fn session_count(&self) -> usize {
        self.sessions.len()
    }

    /// 查询某个 peer 的下一条入站消息序号。
    pub fn next_sequence(&self, peer_id: &str) -> Option<u64> {
        self.sessions
            .get(peer_id)
            .map(TransportSession::next_sequence)
    }

    /// 查询某个 peer 当前缓存的未完成字节数。
    pub fn buffered_len(&self, peer_id: &str) -> Option<usize> {
        self.sessions
            .get(peer_id)
            .map(TransportSession::buffered_len)
    }
}

/// 将同步引擎生成的出站消息编码为传输帧。
pub fn encode_outbound_frames(outbound: &[OutboundEnvelope]) -> P2pResult<Vec<OutboundFrame>> {
    outbound
        .iter()
        .map(|item| {
            let bytes = FramedMessageCodec::encode_frame(&item.message)?;
            Ok(OutboundFrame {
                target_peer_id: item.target_peer_id.clone(),
                bytes,
            })
        })
        .collect()
}

fn merge_report(target: &mut ProcessReport, source: ProcessReport) {
    target.processed = target.processed.saturating_add(source.processed);
    target.outbound.extend(source.outbound);
}

#[cfg(test)]
mod tests {
    use super::{encode_outbound_frames, TransportSession, TransportSessionPool};
    use crate::{
        codec::FramedMessageCodec,
        engine::SyncEngine,
        message::{ChainStatus, NetworkMessage},
    };

    fn local_status(height: u64) -> ChainStatus {
        ChainStatus {
            chain_id: "rustchain-lab-dev".to_string(),
            best_height: height,
            best_hash: format!("0x{height}"),
            difficulty: 2,
            genesis_hash: "0xgenesis".to_string(),
        }
    }

    /// 验证传输会话会缓存半包，并在补齐后交给同步引擎处理。
    #[test]
    fn transport_session_should_process_frame_after_partial_bytes_completed() {
        let mut engine = SyncEngine::new("local-node", local_status(3));
        let mut session = TransportSession::new("peer-a", "/ip4/127.0.0.1/tcp/7001");
        let frame = FramedMessageCodec::encode_frame(&NetworkMessage::Ping {
            nonce: 11,
            timestamp: 22,
        })
        .expect("帧编码应成功");
        let split_at = frame.len() / 2;

        let first = session
            .push_inbound_bytes(&mut engine, &frame[..split_at])
            .expect("半包处理不应报错");
        assert_eq!(first.processed, 0);
        assert_eq!(session.next_sequence(), 1);

        let second = session
            .push_inbound_bytes(&mut engine, &frame[split_at..])
            .expect("补齐后应处理成功");
        assert_eq!(second.processed, 1);
        assert_eq!(session.next_sequence(), 2);
        assert_eq!(second.outbound.len(), 1);
        assert_eq!(
            second.outbound[0].message,
            NetworkMessage::Pong {
                nonce: 11,
                timestamp: 22
            }
        );
    }

    /// 验证粘包中的多条消息会按收到顺序分配连续序号。
    #[test]
    fn transport_session_should_process_sticky_frames_in_order() {
        let mut engine = SyncEngine::new("local-node", local_status(3));
        let mut session = TransportSession::new("peer-a", "/ip4/127.0.0.1/tcp/7001");
        let mut packet = FramedMessageCodec::encode_frame(&NetworkMessage::GetChainStatus)
            .expect("第一帧应成功");
        packet.extend(
            FramedMessageCodec::encode_frame(&NetworkMessage::Ping {
                nonce: 7,
                timestamp: 9,
            })
            .expect("第二帧应成功"),
        );

        let report = session
            .push_inbound_bytes(&mut engine, &packet)
            .expect("粘包处理应成功");

        assert_eq!(report.processed, 2);
        assert_eq!(session.next_sequence(), 3);
        assert!(matches!(
            report.outbound[0].message,
            NetworkMessage::ChainStatus(_)
        ));
        assert!(matches!(
            report.outbound[1].message,
            NetworkMessage::Pong { .. }
        ));
    }

    /// 验证待发送消息可以被编码成传输帧，供真实网络层直接写出。
    #[test]
    fn encode_outbound_frames_should_preserve_target_and_payload() {
        let outbound = vec![crate::engine::OutboundEnvelope {
            target_peer_id: "peer-a".to_string(),
            message: NetworkMessage::GetMempool,
        }];

        let frames = encode_outbound_frames(&outbound).expect("出站帧编码应成功");
        let decoded = FramedMessageCodec::decode_frame(&frames[0].bytes)
            .expect("出站帧解码应成功")
            .expect("应得到完整帧");

        assert_eq!(frames.len(), 1);
        assert_eq!(frames[0].target_peer_id, "peer-a");
        assert_eq!(decoded.message, NetworkMessage::GetMempool);
    }

    /// 验证会话池会按 peer 隔离半包缓存和消息序号。
    #[test]
    fn transport_session_pool_should_keep_peer_buffers_isolated() {
        let mut engine = SyncEngine::new("local-node", local_status(3));
        let mut pool = TransportSessionPool::new();
        let frame_a = FramedMessageCodec::encode_frame(&NetworkMessage::Ping {
            nonce: 1,
            timestamp: 10,
        })
        .expect("peer-a 帧编码应成功");
        let frame_b = FramedMessageCodec::encode_frame(&NetworkMessage::Ping {
            nonce: 2,
            timestamp: 20,
        })
        .expect("peer-b 帧编码应成功");
        let split_at = frame_a.len() / 2;

        let first = pool
            .push_inbound_bytes(
                &mut engine,
                "peer-a",
                "/ip4/127.0.0.1/tcp/7001",
                &frame_a[..split_at],
            )
            .expect("peer-a 半包处理不应报错");
        let second = pool
            .push_inbound_bytes(&mut engine, "peer-b", "/ip4/127.0.0.1/tcp/7002", &frame_b)
            .expect("peer-b 完整帧应处理成功");

        assert_eq!(first.processed, 0);
        assert_eq!(second.processed, 1);
        assert_eq!(pool.session_count(), 2);
        assert_eq!(pool.next_sequence("peer-a"), Some(1));
        assert_eq!(pool.next_sequence("peer-b"), Some(2));
        assert_eq!(pool.buffered_len("peer-a"), Some(split_at));
        assert_eq!(pool.buffered_len("peer-b"), Some(0));

        let tail = pool
            .push_inbound_bytes(
                &mut engine,
                "peer-a",
                "/ip4/127.0.0.1/tcp/7001",
                &frame_a[split_at..],
            )
            .expect("peer-a 补齐后应处理成功");

        assert_eq!(tail.processed, 1);
        assert_eq!(pool.next_sequence("peer-a"), Some(2));
        assert_eq!(pool.buffered_len("peer-a"), Some(0));
    }

    /// 验证会话池可以移除断开的远端会话。
    #[test]
    fn transport_session_pool_should_remove_disconnected_session() {
        let mut engine = SyncEngine::new("local-node", local_status(3));
        let mut pool = TransportSessionPool::new();
        let frame =
            FramedMessageCodec::encode_frame(&NetworkMessage::GetMempool).expect("帧编码应成功");

        pool.push_inbound_bytes(&mut engine, "peer-a", "/ip4/127.0.0.1/tcp/7001", &frame)
            .expect("完整帧应处理成功");

        assert!(pool.remove("peer-a"));
        assert_eq!(pool.session_count(), 0);
        assert_eq!(pool.next_sequence("peer-a"), None);
        assert!(!pool.remove("peer-a"));
    }
}
