use crate::{
    error::CoreError,
    hash::sha256_hex_parts,
    merkle::calculate_merkle_root,
    pow::meets_difficulty,
    transaction::{Transaction, SYSTEM_ADDRESS},
    CoreResult,
};
use chrono::Utc;
use serde::{Deserialize, Serialize};

/// 区块结构，记录一批交易及其共识元数据。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Block {
    /// 区块版本号。
    pub version: u16,
    /// 区块高度。
    pub index: u64,
    /// 区块时间戳。
    pub timestamp: i64,
    /// 区块挖矿难度。
    pub difficulty: u32,
    /// 当前区块打包者（矿工）地址。
    pub miner: String,
    /// 区块内的交易列表。
    pub transactions: Vec<Transaction>,
    /// 前一区块哈希。
    pub previous_hash: String,
    /// 当前区块哈希。
    pub hash: String,
    /// PoW 随机数。
    pub nonce: u64,
    /// 交易列表对应的 Merkle Root。
    pub merkle_root: String,
}

impl Block {
    /// 构造创世区块。这里采用固定内容，便于链初始化和校验。
    pub fn genesis() -> Self {
        let mut block = Self {
            version: 1,
            index: 0,
            timestamp: 0,
            difficulty: 0,
            miner: SYSTEM_ADDRESS.to_string(),
            transactions: Vec::new(),
            previous_hash: "0".to_string(),
            hash: String::new(),
            nonce: 0,
            merkle_root: calculate_merkle_root(&[]),
        };
        block.hash = block.calculate_hash();
        block
    }

    /// 创建一个待挖矿的新区块。
    pub fn new(
        index: u64,
        transactions: Vec<Transaction>,
        previous_hash: impl Into<String>,
        difficulty: u32,
        miner: impl Into<String>,
    ) -> Self {
        let merkle_root = Self::calculate_merkle_root(&transactions);
        let mut block = Self {
            version: 1,
            index,
            timestamp: Utc::now().timestamp(),
            difficulty,
            miner: miner.into(),
            transactions,
            previous_hash: previous_hash.into(),
            hash: String::new(),
            nonce: 0,
            merkle_root,
        };
        block.hash = block.calculate_hash();
        block
    }

    /// 重新计算当前交易列表的 Merkle Root。
    pub fn calculate_merkle_root(transactions: &[Transaction]) -> String {
        let leaves = transactions
            .iter()
            .map(|tx| {
                if tx.id.is_empty() {
                    tx.calculate_id()
                } else {
                    tx.id.clone()
                }
            })
            .collect::<Vec<_>>();

        calculate_merkle_root(&leaves)
    }

    /// 使用指定的 nonce 计算区块哈希。
    pub fn calculate_hash_with_nonce(&self, nonce: u64) -> String {
        let transaction_ids = self
            .transactions
            .iter()
            .map(|tx| {
                if tx.id.is_empty() {
                    tx.calculate_id()
                } else {
                    tx.id.clone()
                }
            })
            .collect::<Vec<_>>()
            .join(",");

        sha256_hex_parts(&[
            self.version.to_string().as_bytes(),
            self.index.to_string().as_bytes(),
            self.timestamp.to_string().as_bytes(),
            self.difficulty.to_string().as_bytes(),
            self.miner.as_bytes(),
            transaction_ids.as_bytes(),
            self.previous_hash.as_bytes(),
            nonce.to_string().as_bytes(),
            self.merkle_root.as_bytes(),
        ])
    }

    /// 基于当前 nonce 重新计算区块哈希。
    pub fn calculate_hash(&self) -> String {
        self.calculate_hash_with_nonce(self.nonce)
    }

    /// 执行 PoW 挖矿，直到满足难度要求。
    pub fn mine(&mut self, difficulty: u32) {
        loop {
            let candidate_hash = self.calculate_hash_with_nonce(self.nonce);
            if meets_difficulty(&candidate_hash, difficulty) {
                self.hash = candidate_hash;
                break;
            }
            self.nonce = self.nonce.saturating_add(1);
        }
    }

    /// 校验区块自身的哈希、Merkle Root 和 PoW。
    pub fn validate_integrity(&self, skip_pow: bool) -> CoreResult<()> {
        let expected_merkle_root = Self::calculate_merkle_root(&self.transactions);
        if self.merkle_root != expected_merkle_root {
            return Err(CoreError::InvalidMerkleRoot { index: self.index });
        }

        let expected_hash = self.calculate_hash();
        if self.hash != expected_hash {
            return Err(CoreError::InvalidBlockHash { index: self.index });
        }

        if !skip_pow && !meets_difficulty(&self.hash, self.difficulty) {
            return Err(CoreError::InvalidProofOfWork { index: self.index });
        }

        for tx in &self.transactions {
            tx.validate_basic()?;
        }

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::transaction::Transaction;

    /// 验证区块在挖矿后能满足难度要求，并通过完整性校验。
    #[test]
    fn mined_block_should_pass_integrity_check() {
        let tx = Transaction::new("alice", "bob", 10, None);
        let mut block = Block::new(1, vec![tx], "prev-hash", 1, "miner-1");
        block.mine(1);

        assert!(block.validate_integrity(false).is_ok());
    }
}
