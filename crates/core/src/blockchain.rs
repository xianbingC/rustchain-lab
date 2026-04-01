use crate::{
    block::Block,
    error::CoreError,
    transaction::Transaction,
    CoreResult,
};
use std::collections::HashMap;

/// 区块链聚合结构，维护主链、交易池和已连接节点信息。
#[derive(Debug, Clone)]
pub struct Blockchain {
    /// 当前主链。
    pub chain: Vec<Block>,
    /// 尚未打包的交易池。
    pub pending_transactions: Vec<Transaction>,
    /// 已知节点列表。
    pub peers: Vec<String>,
    /// 当前 PoW 难度。
    pub difficulty: u32,
    /// 出块奖励。
    pub mining_reward: u64,
}

impl Default for Blockchain {
    fn default() -> Self {
        Self::new(2, 50)
    }
}

impl Blockchain {
    /// 初始化新区块链，并自动创建创世区块。
    pub fn new(difficulty: u32, mining_reward: u64) -> Self {
        Self {
            chain: vec![Block::genesis()],
            pending_transactions: Vec::new(),
            peers: Vec::new(),
            difficulty,
            mining_reward,
        }
    }

    /// 返回最新区块引用。
    pub fn latest_block(&self) -> CoreResult<&Block> {
        self.chain.last().ok_or(CoreError::EmptyChain)
    }

    /// 添加一个已知对等节点。
    pub fn add_peer(&mut self, peer: impl Into<String>) {
        let peer = peer.into();
        if !peer.trim().is_empty() && !self.peers.contains(&peer) {
            self.peers.push(peer);
        }
    }

    /// 将合法交易加入交易池。
    pub fn add_transaction(&mut self, transaction: Transaction) -> CoreResult<()> {
        transaction.validate_basic()?;
        if transaction.is_system() {
            return Err(CoreError::ReservedSystemAddress);
        }

        let mut balances = self.balances();
        self.apply_transactions_to_balances(&self.pending_transactions, &mut balances)?;
        self.apply_transaction_to_balances(&transaction, &mut balances)?;

        self.pending_transactions.push(transaction);
        Ok(())
    }

    /// 将当前交易池打包为新区块，并发放挖矿奖励。
    pub fn mine_pending_transactions(&mut self, miner_address: impl Into<String>) -> CoreResult<Block> {
        let miner_address = miner_address.into();
        let reward_tx = Transaction::system(miner_address, self.mining_reward, None);

        let mut block_transactions = self.pending_transactions.clone();
        block_transactions.push(reward_tx);

        let previous_block = self.latest_block()?;
        let mut candidate_block = Block::new(
            previous_block.index + 1,
            block_transactions,
            previous_block.hash.clone(),
        );
        candidate_block.mine(self.difficulty);

        self.validate_next_block(&candidate_block)?;
        self.chain.push(candidate_block.clone());
        self.pending_transactions.clear();

        Ok(candidate_block)
    }

    /// 根据当前主链计算账户余额快照。
    pub fn balances(&self) -> HashMap<String, u64> {
        let mut balances = HashMap::new();

        for block in &self.chain {
            let _ = self.apply_transactions_to_balances(&block.transactions, &mut balances);
        }

        balances
    }

    /// 校验一整个区块链实例是否合法。
    pub fn validate_chain(&self) -> CoreResult<()> {
        let first_block = self.chain.first().ok_or(CoreError::EmptyChain)?;
        if first_block.hash != Block::genesis().hash || first_block != &Block::genesis() {
            return Err(CoreError::InvalidGenesisBlock);
        }

        let mut balances = HashMap::new();

        for (index, block) in self.chain.iter().enumerate() {
            let is_genesis = index == 0;
            block.validate_integrity(self.difficulty, is_genesis)?;

            if !is_genesis {
                let previous = &self.chain[index - 1];

                if block.index != previous.index + 1 {
                    return Err(CoreError::InvalidBlockIndex {
                        expected: previous.index + 1,
                        actual: block.index,
                    });
                }

                if block.previous_hash != previous.hash {
                    return Err(CoreError::InvalidPreviousHash { index: block.index });
                }
            }

            self.apply_transactions_to_balances(&block.transactions, &mut balances)?;
        }

        Ok(())
    }

    /// 校验一个候选新区块是否可以追加到当前主链。
    pub fn validate_next_block(&self, block: &Block) -> CoreResult<()> {
        let latest = self.latest_block()?;
        block.validate_integrity(self.difficulty, false)?;

        if block.index != latest.index + 1 {
            return Err(CoreError::InvalidBlockIndex {
                expected: latest.index + 1,
                actual: block.index,
            });
        }

        if block.previous_hash != latest.hash {
            return Err(CoreError::InvalidPreviousHash { index: block.index });
        }

        let mut balances = self.balances();
        self.apply_transactions_to_balances(&block.transactions, &mut balances)?;

        Ok(())
    }

    /// 顺序执行交易对余额的影响，保证同一区块内的余额检查是有状态的。
    fn apply_transactions_to_balances(
        &self,
        transactions: &[Transaction],
        balances: &mut HashMap<String, u64>,
    ) -> CoreResult<()> {
        for tx in transactions {
            self.apply_transaction_to_balances(tx, balances)?;
        }

        Ok(())
    }

    /// 将单笔交易应用到余额快照中。
    fn apply_transaction_to_balances(
        &self,
        transaction: &Transaction,
        balances: &mut HashMap<String, u64>,
    ) -> CoreResult<()> {
        transaction.validate_basic()?;

        if !transaction.is_system() {
            let available = balances.get(&transaction.from).copied().unwrap_or(0);
            if available < transaction.amount {
                return Err(CoreError::InsufficientBalance {
                    address: transaction.from.clone(),
                    needed: transaction.amount,
                    available,
                });
            }

            balances.insert(transaction.from.clone(), available - transaction.amount);
        }

        let recipient_balance = balances.get(&transaction.to).copied().unwrap_or(0);
        balances.insert(
            transaction.to.clone(),
            recipient_balance.saturating_add(transaction.amount),
        );

        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证系统奖励出块后，矿工余额能够正确增加。
    #[test]
    fn mining_reward_should_increase_miner_balance() {
        let mut blockchain = Blockchain::new(1, 50);
        let mined_block = blockchain
            .mine_pending_transactions("miner-1")
            .expect("挖矿应当成功");

        assert_eq!(mined_block.index, 1);
        assert_eq!(blockchain.pending_transactions.len(), 0);
        assert_eq!(blockchain.balances().get("miner-1").copied(), Some(50));
    }

    /// 验证在已有余额的前提下，普通转账和整链校验都能通过。
    #[test]
    fn transfer_flow_should_keep_chain_valid() {
        let mut blockchain = Blockchain::new(1, 50);
        blockchain
            .mine_pending_transactions("alice")
            .expect("第一次挖矿应当成功");

        let tx = Transaction::new("alice", "bob", 20, None);
        blockchain.add_transaction(tx).expect("交易应当可以入池");
        blockchain
            .mine_pending_transactions("miner-2")
            .expect("第二次挖矿应当成功");

        assert_eq!(blockchain.balances().get("alice").copied(), Some(30));
        assert_eq!(blockchain.balances().get("bob").copied(), Some(20));
        assert!(blockchain.validate_chain().is_ok());
    }

    /// 验证外部接口不能直接提交系统交易。
    #[test]
    fn external_system_transaction_should_be_rejected() {
        let mut blockchain = Blockchain::new(1, 50);
        let tx = Transaction::system("mallory", 999, None);
        let result = blockchain.add_transaction(tx);

        assert_eq!(result, Err(CoreError::ReservedSystemAddress));
    }
}
