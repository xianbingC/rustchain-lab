use crate::{
    error::{P2pError, P2pResult},
    message::NetworkMessage,
};
use std::{
    collections::{BTreeMap, VecDeque},
    time::{SystemTime, UNIX_EPOCH},
};

/// 带序号的消息，用于顺序处理。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SequencedMessage {
    /// 发送方节点 ID。
    pub peer_id: String,
    /// 全局递增序号（在单一连接内递增）。
    pub sequence: u64,
    /// 接收时间戳（秒）。
    pub received_at_secs: u64,
    /// 实际业务消息。
    pub message: NetworkMessage,
}

impl SequencedMessage {
    /// 构造一条带当前时间戳的序列化消息。
    pub fn new(peer_id: impl Into<String>, sequence: u64, message: NetworkMessage) -> Self {
        let received_at_secs = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_secs())
            .unwrap_or(0);

        Self {
            peer_id: peer_id.into(),
            sequence,
            received_at_secs,
            message,
        }
    }
}

/// 顺序消息队列，确保消息按序交付。
#[derive(Debug)]
pub struct OrderedMessageQueue {
    next_expected: u64,
    pending: BTreeMap<u64, SequencedMessage>,
    ready: VecDeque<SequencedMessage>,
}

impl OrderedMessageQueue {
    /// 创建顺序队列，并指定期望起始序号。
    pub fn new(next_expected: u64) -> Self {
        Self {
            next_expected,
            pending: BTreeMap::new(),
            ready: VecDeque::new(),
        }
    }

    /// 返回当前期望序号。
    pub fn next_expected(&self) -> u64 {
        self.next_expected
    }

    /// 将新消息放入队列，按序整理到可消费队列。
    pub fn push(&mut self, msg: SequencedMessage) -> P2pResult<()> {
        if msg.sequence < self.next_expected {
            return Err(P2pError::StaleSequence {
                seq: msg.sequence,
                expected: self.next_expected,
            });
        }

        self.pending.entry(msg.sequence).or_insert(msg);
        self.collect_ready();
        Ok(())
    }

    /// 弹出一条已经就绪的消息。
    pub fn pop_ready(&mut self) -> Option<SequencedMessage> {
        self.ready.pop_front()
    }

    /// 批量弹出所有已就绪消息。
    pub fn pop_all_ready(&mut self) -> Vec<SequencedMessage> {
        self.ready.drain(..).collect()
    }

    /// 当前 pending 中的消息数量。
    pub fn pending_len(&self) -> usize {
        self.pending.len()
    }

    /// 当前 ready 中的消息数量。
    pub fn ready_len(&self) -> usize {
        self.ready.len()
    }

    /// 收集连续序号消息到 ready 队列。
    fn collect_ready(&mut self) {
        while let Some(msg) = self.pending.remove(&self.next_expected) {
            self.ready.push_back(msg);
            self.next_expected = self.next_expected.saturating_add(1);
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证乱序输入最终可以按序输出。
    #[test]
    fn out_of_order_messages_should_be_processed_in_order() {
        let mut queue = OrderedMessageQueue::new(10);
        queue
            .push(SequencedMessage::new(
                "peer-a",
                11,
                NetworkMessage::GetMempool,
            ))
            .expect("入队应当成功");
        queue
            .push(SequencedMessage::new(
                "peer-a",
                10,
                NetworkMessage::GetChainStatus,
            ))
            .expect("入队应当成功");

        let ready = queue.pop_all_ready();
        assert_eq!(ready.len(), 2);
        assert_eq!(ready[0].sequence, 10);
        assert_eq!(ready[1].sequence, 11);
        assert_eq!(queue.next_expected(), 12);
    }

    /// 验证旧序号消息会被拒绝。
    #[test]
    fn stale_message_should_be_rejected() {
        let mut queue = OrderedMessageQueue::new(5);
        let result = queue.push(SequencedMessage::new(
            "peer-a",
            4,
            NetworkMessage::GetChainStatus,
        ));

        assert_eq!(
            result,
            Err(P2pError::StaleSequence {
                seq: 4,
                expected: 5,
            })
        );
    }
}
