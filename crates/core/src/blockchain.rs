use crate::{
    block::Block,
    error::CoreError,
    transaction::{Transaction, TransactionKind},
    CoreResult,
};
use rustchain_vm::{compiler::compile, runtime::Runtime};
use std::collections::HashMap;

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
    /// 出块奖励。
    pub mining_reward: u64,
    /// 目标出块时间（秒），用于后续动态难度调整。
    pub target_block_time_secs: u64,
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
            mining_reward,
            target_block_time_secs: 10,
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
        let mut candidate_block = Block::new(
            previous_block.index + 1,
            block_transactions,
            previous_block.hash.clone(),
            self.difficulty,
            miner_address,
        );
        candidate_block.mine(self.difficulty);

        self.validate_next_block(&candidate_block)?;
        self.apply_contract_state_transitions(&candidate_block.transactions)?;
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

                if block.difficulty != self.difficulty {
                    return Err(CoreError::InvalidBlockDifficulty {
                        index: block.index,
                        expected: self.difficulty,
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

        if block.difficulty != self.difficulty {
            return Err(CoreError::InvalidBlockDifficulty {
                index: block.index,
                expected: self.difficulty,
                actual: block.difficulty,
            });
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
}
