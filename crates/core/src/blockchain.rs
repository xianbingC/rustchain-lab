use crate::{
    block::Block,
    error::CoreError,
    transaction::{Transaction, TransactionKind},
    CoreResult,
};
use rustchain_vm::{compiler::compile, runtime::Runtime};
use std::collections::HashMap;

/// 难度调整间隔默认值（按区块高度计）。
const DEFAULT_DIFFICULTY_ADJUSTMENT_INTERVAL: u64 = 10;

/// 区块链聚合结构，维护主链、交易池和已连接节点信息。
#[derive(Debug, Clone)]
pub struct Blockchain {
    /// 链标识，用于区分不同部署环境。
    pub chain_id: String,
    /// 当前主链。
    pub chain: Vec<Block>,
    /// 尚未打包的交易池。
    pub pending_transactions: Vec<Transaction>,
    /// 已知节点列表。
    pub peers: Vec<String>,
    /// 当前 PoW 难度。
    pub difficulty: u32,
    /// 初始难度，作为第一块（非创世区块）难度基线。
    initial_difficulty: u32,
    /// 出块奖励。
    pub mining_reward: u64,
    /// 目标出块时间（秒），用于动态难度调整。
    pub target_block_time_secs: u64,
    /// 难度调整窗口（每隔 N 个区块尝试调整一次）。
    pub difficulty_adjustment_interval: u64,
    /// 合约状态快照，key 为合约地址。
    pub contract_states: HashMap<String, HashMap<String, i64>>,
    /// 合约事件日志，key 为合约地址。
    pub contract_events: HashMap<String, Vec<String>>,
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
            chain_id: "rustchain-lab-dev".to_string(),
            chain: vec![Block::genesis()],
            pending_transactions: Vec::new(),
            peers: Vec::new(),
            difficulty,
            initial_difficulty: difficulty,
            mining_reward,
            target_block_time_secs: 10,
            difficulty_adjustment_interval: DEFAULT_DIFFICULTY_ADJUSTMENT_INTERVAL,
            contract_states: HashMap::new(),
            contract_events: HashMap::new(),
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

    /// 查询指定合约地址的最新状态快照。
    pub fn contract_state_snapshot(&self, contract_address: &str) -> Option<HashMap<String, i64>> {
        self.contract_states.get(contract_address).cloned()
    }

    /// 查询指定合约地址累计事件。
    pub fn contract_events_snapshot(&self, contract_address: &str) -> Vec<String> {
        self.contract_events
            .get(contract_address)
            .cloned()
            .unwrap_or_default()
    }

    /// 将合法交易加入交易池。
    pub fn add_transaction(&mut self, transaction: Transaction) -> CoreResult<()> {
        transaction.validate_for_chain()?;
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
    pub fn mine_pending_transactions(
        &mut self,
        miner_address: impl Into<String>,
    ) -> CoreResult<Block> {
        let miner_address = miner_address.into();
        let reward_tx = Transaction::system(miner_address.clone(), self.mining_reward, None);

        let mut block_transactions = self.pending_transactions.clone();
        block_transactions.push(reward_tx);

        let previous_block = self.latest_block()?;
        let expected_difficulty = self.expected_difficulty_for_height(previous_block.index + 1)?;
        let mut candidate_block = Block::new(
            previous_block.index + 1,
            block_transactions,
            previous_block.hash.clone(),
            expected_difficulty,
            miner_address,
        );
        candidate_block.mine(expected_difficulty);

        self.validate_next_block(&candidate_block)?;
        self.apply_contract_state_transitions(&candidate_block.transactions)?;
        self.chain.push(candidate_block.clone());
        self.pending_transactions.clear();
        self.refresh_next_difficulty_cache();

        Ok(candidate_block)
    }

    /// 接收外部同步区块并追加到当前主链。
    pub fn append_external_block(&mut self, block: Block) -> CoreResult<()> {
        self.validate_next_block(&block)?;
        self.apply_contract_state_transitions(&block.transactions)?;
        self.chain.push(block.clone());
        self.refresh_next_difficulty_cache();

        // 将已确认区块中的交易从待打包池移除，避免重复打包。
        let confirmed_ids = block
            .transactions
            .iter()
            .map(|tx| tx.id.clone())
            .collect::<std::collections::HashSet<_>>();
        self.pending_transactions
            .retain(|pending| !confirmed_ids.contains(&pending.id));
        Ok(())
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
            block.validate_integrity(is_genesis)?;

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

                let expected_difficulty = self.expected_difficulty_for_height(block.index)?;
                if block.difficulty != expected_difficulty {
                    return Err(CoreError::InvalidBlockDifficulty {
                        index: block.index,
                        expected: expected_difficulty,
                        actual: block.difficulty,
                    });
                }
            }

            self.apply_transactions_to_balances(&block.transactions, &mut balances)?;
        }

        Ok(())
    }

    /// 校验一个候选新区块是否可以追加到当前主链。
    pub fn validate_next_block(&self, block: &Block) -> CoreResult<()> {
        let latest = self.latest_block()?;
        block.validate_integrity(false)?;

        if block.index != latest.index + 1 {
            return Err(CoreError::InvalidBlockIndex {
                expected: latest.index + 1,
                actual: block.index,
            });
        }

        if block.previous_hash != latest.hash {
            return Err(CoreError::InvalidPreviousHash { index: block.index });
        }

        let expected_difficulty = self.expected_difficulty_for_height(block.index)?;
        if block.difficulty != expected_difficulty {
            return Err(CoreError::InvalidBlockDifficulty {
                index: block.index,
                expected: expected_difficulty,
                actual: block.difficulty,
            });
        }

        let mut balances = self.balances();
        self.apply_transactions_to_balances(&block.transactions, &mut balances)?;

        Ok(())
    }

    /// 计算指定高度区块的期望难度。
    fn expected_difficulty_for_height(&self, height: u64) -> CoreResult<u32> {
        if height == 0 {
            return Ok(0);
        }
        if height == 1 {
            return Ok(self.initial_difficulty);
        }

        let previous_position = (height - 1) as usize;
        let previous_block = self
            .chain
            .get(previous_position)
            .ok_or(CoreError::EmptyChain)?;
        if self.difficulty_adjustment_interval <= 1
            || height % self.difficulty_adjustment_interval != 0
        {
            return Ok(previous_block.difficulty);
        }

        let interval = self.difficulty_adjustment_interval;
        if height < interval {
            return Ok(previous_block.difficulty);
        }

        let start_position = (height - interval) as usize;
        let Some(start_block) = self.chain.get(start_position) else {
            return Ok(previous_block.difficulty);
        };

        // 创世区块时间戳固定为 0，不参与首轮难度估算。
        if start_block.index == 0 {
            return Ok(previous_block.difficulty);
        }

        let elapsed_secs = previous_block
            .timestamp
            .saturating_sub(start_block.timestamp)
            .max(1) as u64;
        let expected_elapsed_secs = self
            .target_block_time_secs
            .saturating_mul(interval.saturating_sub(1).max(1));

        Ok(Self::adjust_difficulty_with_elapsed(
            previous_block.difficulty,
            elapsed_secs,
            expected_elapsed_secs,
        ))
    }

    /// 基于时间窗口调整难度，快则上调、慢则下调。
    fn adjust_difficulty_with_elapsed(
        previous_difficulty: u32,
        elapsed_secs: u64,
        expected_elapsed_secs: u64,
    ) -> u32 {
        if expected_elapsed_secs == 0 {
            return previous_difficulty;
        }

        let lower_bound = expected_elapsed_secs.saturating_div(2).max(1);
        let upper_bound = expected_elapsed_secs.saturating_mul(2);
        if elapsed_secs < lower_bound {
            previous_difficulty.saturating_add(1)
        } else if elapsed_secs > upper_bound {
            previous_difficulty.saturating_sub(1)
        } else {
            previous_difficulty
        }
    }

    /// 刷新缓存难度，保证接口对外展示当前下一块的目标难度。
    fn refresh_next_difficulty_cache(&mut self) {
        if let Ok(next_difficulty) = self
            .latest_block()
            .and_then(|latest| self.expected_difficulty_for_height(latest.index + 1))
        {
            self.difficulty = next_difficulty;
        }
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
        transaction.validate_for_chain()?;
        self.validate_transaction_payload(transaction)?;

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

    /// 校验并执行交易中附带的合约脚本载荷。
    fn validate_transaction_payload(&self, transaction: &Transaction) -> CoreResult<()> {
        if !is_contract_transaction(transaction) {
            return Ok(());
        }

        let Some(raw_payload) = transaction.payload.as_ref() else {
            return Ok(());
        };

        if raw_payload.is_empty() {
            return Ok(());
        }

        let source = std::str::from_utf8(raw_payload).map_err(|error| {
            CoreError::ContractPayloadEncodingInvalid {
                tx_id: transaction.id.clone(),
                reason: error.to_string(),
            }
        })?;

        if source.trim().is_empty() {
            return Ok(());
        }

        let program = compile(source).map_err(|error| CoreError::ContractCompileFailed {
            tx_id: transaction.id.clone(),
            reason: error.to_string(),
        })?;
        let initial_state = self
            .contract_states
            .get(&transaction.to)
            .cloned()
            .unwrap_or_default();
        let mut runtime = Runtime::from_state(initial_state);
        runtime
            .execute(&program)
            .map_err(|error| CoreError::ContractExecutionFailed {
                tx_id: transaction.id.clone(),
                reason: error.to_string(),
            })?;
        Ok(())
    }

    /// 在区块确认后推进合约状态与事件日志。
    fn apply_contract_state_transitions(&mut self, transactions: &[Transaction]) -> CoreResult<()> {
        for transaction in transactions {
            if !is_contract_transaction(transaction) {
                continue;
            }

            let Some(raw_payload) = transaction.payload.as_ref() else {
                continue;
            };
            if raw_payload.is_empty() {
                continue;
            }

            let source = std::str::from_utf8(raw_payload).map_err(|error| {
                CoreError::ContractPayloadEncodingInvalid {
                    tx_id: transaction.id.clone(),
                    reason: error.to_string(),
                }
            })?;
            if source.trim().is_empty() {
                continue;
            }

            let program = compile(source).map_err(|error| CoreError::ContractCompileFailed {
                tx_id: transaction.id.clone(),
                reason: error.to_string(),
            })?;
            let initial_state = self
                .contract_states
                .get(&transaction.to)
                .cloned()
                .unwrap_or_default();
            let mut runtime = Runtime::from_state(initial_state);
            runtime
                .execute(&program)
                .map_err(|error| CoreError::ContractExecutionFailed {
                    tx_id: transaction.id.clone(),
                    reason: error.to_string(),
                })?;

            self.contract_states
                .insert(transaction.to.clone(), runtime.state().clone());
            if !runtime.events().is_empty() {
                self.contract_events
                    .entry(transaction.to.clone())
                    .or_default()
                    .extend(runtime.events().iter().cloned());
            }
        }

        Ok(())
    }
}

/// 判断交易是否属于合约执行语义。
fn is_contract_transaction(transaction: &Transaction) -> bool {
    matches!(
        transaction.kind,
        TransactionKind::ContractDeploy | TransactionKind::ContractCall
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use rustchain_crypto::wallet::create_wallet;

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
        let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应当成功");
        let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应当成功");
        let mut blockchain = Blockchain::new(1, 50);
        blockchain
            .mine_pending_transactions(alice_wallet.address.clone())
            .expect("第一次挖矿应当成功");

        let mut tx = Transaction::new(
            alice_wallet.address.clone(),
            bob_wallet.address.clone(),
            20,
            None,
        );
        tx.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
            .expect("交易签名应当成功");
        blockchain.add_transaction(tx).expect("交易应当可以入池");
        blockchain
            .mine_pending_transactions("miner-2")
            .expect("第二次挖矿应当成功");

        assert_eq!(
            blockchain.balances().get(&alice_wallet.address).copied(),
            Some(30)
        );
        assert_eq!(
            blockchain.balances().get(&bob_wallet.address).copied(),
            Some(20)
        );
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

    /// 验证候选区块难度与链配置不一致时会被拒绝。
    #[test]
    fn candidate_block_with_invalid_difficulty_should_be_rejected() {
        let blockchain = Blockchain::new(2, 50);
        let latest = blockchain.latest_block().expect("应当存在创世区块");
        let mut block = Block::new(1, Vec::new(), latest.hash.clone(), 1, "miner-1");
        block.mine(1);

        let result = blockchain.validate_next_block(&block);
        assert_eq!(
            result,
            Err(CoreError::InvalidBlockDifficulty {
                index: 1,
                expected: 2,
                actual: 1,
            })
        );
    }

    /// 验证携带合法合约脚本的交易可以入池。
    #[test]
    fn transaction_with_valid_contract_payload_should_be_accepted() {
        let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应当成功");
        let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应当成功");
        let mut blockchain = Blockchain::new(1, 50);
        blockchain
            .mine_pending_transactions(alice_wallet.address.clone())
            .expect("第一次挖矿应当成功");

        let mut tx = Transaction::new_with_kind(
            TransactionKind::ContractCall,
            alice_wallet.address.clone(),
            bob_wallet.address.clone(),
            10,
            0,
            Some(b"LOAD_CONST 1\nSTORE x\nHALT\n".to_vec()),
        );
        tx.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
            .expect("交易签名应当成功");

        let result = blockchain.add_transaction(tx);
        assert!(result.is_ok());
    }

    /// 验证合约编译失败的交易会被拒绝。
    #[test]
    fn transaction_with_invalid_contract_payload_should_be_rejected() {
        let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应当成功");
        let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应当成功");
        let mut blockchain = Blockchain::new(1, 50);
        blockchain
            .mine_pending_transactions(alice_wallet.address.clone())
            .expect("第一次挖矿应当成功");

        let mut tx = Transaction::new_with_kind(
            TransactionKind::ContractCall,
            alice_wallet.address.clone(),
            bob_wallet.address.clone(),
            10,
            0,
            Some(b"WARP 1\n".to_vec()),
        );
        tx.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
            .expect("交易签名应当成功");

        let result = blockchain.add_transaction(tx);
        assert!(matches!(
            result,
            Err(CoreError::ContractCompileFailed { .. })
        ));
    }

    /// 验证合约运行时失败的交易会被拒绝。
    #[test]
    fn transaction_with_runtime_error_payload_should_be_rejected() {
        let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应当成功");
        let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应当成功");
        let mut blockchain = Blockchain::new(1, 50);
        blockchain
            .mine_pending_transactions(alice_wallet.address.clone())
            .expect("第一次挖矿应当成功");

        let mut tx = Transaction::new_with_kind(
            TransactionKind::ContractCall,
            alice_wallet.address.clone(),
            bob_wallet.address.clone(),
            10,
            0,
            Some(b"LOAD_CONST 7\nLOAD_CONST 0\nDIV\nHALT\n".to_vec()),
        );
        tx.sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
            .expect("交易签名应当成功");

        let result = blockchain.add_transaction(tx);
        assert!(matches!(
            result,
            Err(CoreError::ContractExecutionFailed { .. })
        ));
    }

    /// 验证合约调用在出块后会更新同一合约的状态与事件。
    #[test]
    fn contract_state_and_events_should_update_after_mining() {
        let (alice_wallet, alice_key_pair) = create_wallet("alice-pass").expect("创建钱包应当成功");
        let mut blockchain = Blockchain::new(1, 50);
        let contract_address = "contract-counter";
        blockchain
            .mine_pending_transactions(alice_wallet.address.clone())
            .expect("第一次挖矿应当成功");

        let mut init_tx = Transaction::new_with_kind(
            TransactionKind::ContractCall,
            alice_wallet.address.clone(),
            contract_address,
            1,
            1,
            Some(b"LOAD_CONST 1\nSTORE counter\nEMIT \"init\"\nHALT\n".to_vec()),
        );
        init_tx
            .sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
            .expect("交易签名应当成功");
        blockchain
            .add_transaction(init_tx)
            .expect("初始化交易应当入池");
        blockchain
            .mine_pending_transactions("miner-2")
            .expect("出块应当成功");

        let mut inc_tx = Transaction::new_with_kind(
            TransactionKind::ContractCall,
            alice_wallet.address.clone(),
            contract_address,
            1,
            2,
            Some(b"LOAD counter\nLOAD_CONST 1\nADD\nSTORE counter\nEMIT \"inc\"\nHALT\n".to_vec()),
        );
        inc_tx
            .sign_with_private_key(&alice_key_pair.private_key, &alice_key_pair.public_key)
            .expect("交易签名应当成功");
        blockchain
            .add_transaction(inc_tx)
            .expect("递增交易应当入池");
        blockchain
            .mine_pending_transactions("miner-3")
            .expect("出块应当成功");

        let state = blockchain
            .contract_state_snapshot(contract_address)
            .expect("应存在合约状态");
        assert_eq!(state.get("counter"), Some(&2));
        assert_eq!(
            blockchain.contract_events_snapshot(contract_address),
            vec!["init".to_string(), "inc".to_string()]
        );
    }

    /// 验证可以接收并追加外部区块。
    #[test]
    fn append_external_block_should_work() {
        let mut local_chain = Blockchain::new(1, 50);
        let mut remote_chain = Blockchain::new(1, 50);

        let block = remote_chain
            .mine_pending_transactions("remote-miner")
            .expect("远端出块应成功");
        local_chain
            .append_external_block(block.clone())
            .expect("追加外部区块应成功");

        assert_eq!(local_chain.chain.len(), 2);
        assert_eq!(
            local_chain.latest_block().expect("应存在最新区块").hash,
            block.hash
        );
    }

    /// 验证动态难度会在调整窗口命中时上调。
    #[test]
    fn difficulty_should_increase_on_fast_blocks_at_adjustment_boundary() {
        let mut blockchain = Blockchain::new(1, 50);
        blockchain.target_block_time_secs = 100;
        blockchain.difficulty_adjustment_interval = 2;

        let block1 = blockchain
            .mine_pending_transactions("miner-1")
            .expect("第一块挖矿应成功");
        let block2 = blockchain
            .mine_pending_transactions("miner-2")
            .expect("第二块挖矿应成功");
        let block3 = blockchain
            .mine_pending_transactions("miner-3")
            .expect("第三块挖矿应成功");
        let block4 = blockchain
            .mine_pending_transactions("miner-4")
            .expect("第四块挖矿应成功");

        assert_eq!(block1.difficulty, 1);
        assert_eq!(block2.difficulty, 1);
        assert_eq!(block3.difficulty, 1);
        assert_eq!(block4.difficulty, 2);
        assert_eq!(blockchain.difficulty, 2);
    }

    /// 验证难度调整函数在快慢窗口下能正确上调或下调。
    #[test]
    fn adjust_difficulty_with_elapsed_should_follow_window_rule() {
        assert_eq!(Blockchain::adjust_difficulty_with_elapsed(2, 4, 20), 3);
        assert_eq!(Blockchain::adjust_difficulty_with_elapsed(2, 50, 20), 1);
        assert_eq!(Blockchain::adjust_difficulty_with_elapsed(2, 15, 20), 2);
    }
}
