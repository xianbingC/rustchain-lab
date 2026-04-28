use crate::{
    error::{P2pError, P2pResult},
    message::NetworkMessage,
};

/// 默认最大帧长度，避免异常节点发送超大消息拖垮内存。
pub const DEFAULT_MAX_FRAME_LEN: usize = 1024 * 1024;

/// 网络消息编解码器，统一处理序列化与基础校验。
pub struct MessageCodec;

impl MessageCodec {
    /// 将消息编码为字节流，并提前执行基础校验。
    pub fn encode(message: &NetworkMessage) -> P2pResult<Vec<u8>> {
        message
            .validate_basic()
            .map_err(|error| P2pError::InvalidMessage(error.to_string()))?;

        bincode::serialize(message).map_err(|error| P2pError::Serialize(error.to_string()))
    }

    /// 将字节流解码为消息，并执行基础校验。
    pub fn decode(data: &[u8]) -> P2pResult<NetworkMessage> {
        let message: NetworkMessage =
            bincode::deserialize(data).map_err(|error| P2pError::Deserialize(error.to_string()))?;

        message
            .validate_basic()
            .map_err(|error| P2pError::InvalidMessage(error.to_string()))?;
        Ok(message)
    }
}

/// 解码后的完整帧。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DecodedFrame {
    /// 帧内网络消息。
    pub message: NetworkMessage,
    /// 本次从缓冲区消费的字节数。
    pub consumed: usize,
}

/// 长度前缀帧编解码器，供真实传输层复用。
pub struct FramedMessageCodec;

impl FramedMessageCodec {
    /// 将网络消息编码为 `4 字节长度 + bincode 载荷` 的传输帧。
    pub fn encode_frame(message: &NetworkMessage) -> P2pResult<Vec<u8>> {
        Self::encode_frame_with_max_len(message, DEFAULT_MAX_FRAME_LEN)
    }

    /// 使用自定义最大长度编码传输帧，便于测试和不同网络配置复用。
    pub fn encode_frame_with_max_len(
        message: &NetworkMessage,
        max_frame_len: usize,
    ) -> P2pResult<Vec<u8>> {
        let payload = MessageCodec::encode(message)?;
        if payload.len() > max_frame_len {
            return Err(P2pError::InvalidArgument(format!(
                "消息帧过大: len={}, max={max_frame_len}",
                payload.len()
            )));
        }

        let mut frame = Vec::with_capacity(4 + payload.len());
        frame.extend_from_slice(&(payload.len() as u32).to_be_bytes());
        frame.extend_from_slice(&payload);
        Ok(frame)
    }

    /// 尝试从缓冲区解码一帧；数据不完整时返回 None。
    pub fn decode_frame(data: &[u8]) -> P2pResult<Option<DecodedFrame>> {
        Self::decode_frame_with_max_len(data, DEFAULT_MAX_FRAME_LEN)
    }

    /// 使用自定义最大长度解码传输帧。
    pub fn decode_frame_with_max_len(
        data: &[u8],
        max_frame_len: usize,
    ) -> P2pResult<Option<DecodedFrame>> {
        if data.len() < 4 {
            return Ok(None);
        }

        let payload_len = u32::from_be_bytes([data[0], data[1], data[2], data[3]]) as usize;
        if payload_len > max_frame_len {
            return Err(P2pError::InvalidArgument(format!(
                "消息帧过大: len={payload_len}, max={max_frame_len}"
            )));
        }

        let frame_len = 4usize.saturating_add(payload_len);
        if data.len() < frame_len {
            return Ok(None);
        }

        let message = MessageCodec::decode(&data[4..frame_len])?;
        Ok(Some(DecodedFrame {
            message,
            consumed: frame_len,
        }))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::message::{ChainStatus, NetworkMessage};

    /// 验证合法消息可以完成编解码闭环。
    #[test]
    fn message_codec_roundtrip_should_work() {
        let msg = NetworkMessage::ChainStatus(ChainStatus {
            chain_id: "rustchain-lab-dev".to_string(),
            best_height: 12,
            best_hash: "0xabc".to_string(),
            difficulty: 2,
            genesis_hash: "0xgenesis".to_string(),
        });

        let encoded = MessageCodec::encode(&msg).expect("编码应当成功");
        let decoded = MessageCodec::decode(&encoded).expect("解码应当成功");

        assert_eq!(decoded, msg);
    }

    /// 验证非法消息在编码阶段会被拦截。
    #[test]
    fn invalid_message_should_fail_encode() {
        let msg = NetworkMessage::NewBlock { block: Vec::new() };
        let result = MessageCodec::encode(&msg);

        assert!(matches!(result, Err(P2pError::InvalidMessage(_))));
    }

    /// 验证长度前缀帧可以完成消息编解码闭环。
    #[test]
    fn framed_codec_should_roundtrip_message() {
        let msg = NetworkMessage::GetBlocks {
            from_height: 3,
            limit: 16,
        };

        let frame = FramedMessageCodec::encode_frame(&msg).expect("帧编码应成功");
        let decoded = FramedMessageCodec::decode_frame(&frame)
            .expect("帧解码应成功")
            .expect("完整帧应返回消息");

        assert_eq!(decoded.message, msg);
        assert_eq!(decoded.consumed, frame.len());
    }

    /// 验证半包不会报错，而是等待更多字节。
    #[test]
    fn framed_codec_should_wait_for_incomplete_frame() {
        let msg = NetworkMessage::GetMempool;
        let frame = FramedMessageCodec::encode_frame(&msg).expect("帧编码应成功");
        let incomplete = &frame[..frame.len() - 1];

        let decoded = FramedMessageCodec::decode_frame(incomplete).expect("半包解析应成功");

        assert!(decoded.is_none());
    }

    /// 验证超过最大帧长度时会被拒绝。
    #[test]
    fn framed_codec_should_reject_oversized_payload() {
        let msg = NetworkMessage::NewTransaction {
            transaction: vec![1, 2, 3, 4],
        };

        let result = FramedMessageCodec::encode_frame_with_max_len(&msg, 1);

        assert!(matches!(result, Err(P2pError::InvalidArgument(_))));
    }
}
