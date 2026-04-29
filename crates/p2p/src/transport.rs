use crate::{
    codec::{FrameDecodeBuffer, FramedMessageCodec},
    engine::{OutboundEnvelope, ProcessReport, SyncEngine},
    P2pResult,
};
use serde::{Deserialize, Serialize};

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
    use super::{encode_outbound_frames, TransportSession};
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
}
