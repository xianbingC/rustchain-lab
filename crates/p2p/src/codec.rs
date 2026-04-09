use crate::{
    error::{P2pError, P2pResult},
    message::NetworkMessage,
};

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
}
