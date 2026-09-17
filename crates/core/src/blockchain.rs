use crate::{
    block::Block,
    defi_payload::{DefiAction, DefiPayload},
    error::CoreError,
    transaction::{Transaction, TransactionKind},
    CoreResult,
};
use rustchain_apps::defi::{LendingConfig, LendingPool};
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
    /// DeFi 借贷池状态，随区块确认推进。
    pub lending_pool: LendingPool,
}

impl Default for Blockchain {
    fn default() -> Self {
        Self::new(2, 50)
    }
}

impl Blockchain {
    /// 初始化新区块链，并自动创建创世区块。
    pub fn new(difficulty: u32, mining_reward: u64) -> Self {
        Self::new_with_defi_config(difficulty, mining_reward, LendingConfig::default())
    }

    /// 使用自定义 DeFi 借贷参数初始化新区块链。
    pub fn new_with_defi_config(
        difficulty: u32,
        mining_reward: u64,
        defi_config: LendingConfig,
    ) -> Self {
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
            // 借贷池以创世时间起点初始化，链上计息从区块时间戳推进。
            lending_pool: LendingPool::new(defi_config, 0),
        }
    }

    /// 返回最新区块引用。
    pub fn latest_block(&self) -> CoreResult<&Block> {
        self.chain.last().ok_or(CoreError::EmptyChain)
    }

    /// 返回最新已确认区块的挖矿难度。
    pub fn latest_block_difficulty(&self) -> CoreResult<u32> {
        Ok(self.latest_block()?.difficulty)
    }

    /// 返回下一块区块的期望挖矿难度。
    pub fn next_block_expected_difficulty(&self) -> CoreResult<u32> {
        let latest = self.latest_block()?;
        self.expected_difficulty_for_height(latest.index + 1)
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
        self.apply_state_transitions(&candidate_block.transactions)?;
        self.chain.push(candidate_block.clone());
        self.pending_transactions.clear();
        self.refresh_next_difficulty_cache();

        Ok(candidate_block)
    }

    /// 接收外部同步区块并追加到当前主链。
    pub fn append_external_block(&mut self, block: Block) -> CoreResult<()> {
        self.validate_next_block(&block)?;
        self.apply_state_transitions(&block.transactions)?;
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
    ///
    /// DeFi 动作只操作借贷池内部账本，`amount` 表示抵押/借款数量而非转账金额，
    /// 因此不参与账户余额的收支计算。
    fn apply_transaction_to_balances(
        &self,
        transaction: &Transaction,
        balances: &mut HashMap<String, u64>,
    ) -> CoreResult<()> {
        transaction.validate_for_chain()?;
        self.validate_transaction_payload(transaction)?;

        if is_internal_action(transaction) {
            return Ok(());
        }

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

    /// 校验交易中附带的业务载荷（合约脚本或 DeFi 动作）。
    ///
    /// 入池阶段只做"试执行"校验，不落状态；真正的状态推进在出块确认后由
    /// `apply_state_transitions` 完成。
    fn validate_transaction_payload(&self, transaction: &Transaction) -> CoreResult<()> {
        match transaction.kind {
            TransactionKind::ContractDeploy | TransactionKind::ContractCall => {
                self.validate_contract_payload(transaction)
            }
            TransactionKind::DefiAction => self.validate_defi_payload(transaction),
            _ => Ok(()),
        }
    }

    /// 校验合约脚本载荷（UTF-8 文本源码）。
    fn validate_contract_payload(&self, transaction: &Transaction) -> CoreResult<()> {
        let Some(source) = decode_contract_source(transaction)? else {
            return Ok(());
        };

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

    /// 校验 DeFi 载荷结构与业务前置条件。
    ///
    /// 业务校验在借贷池副本上试执行，保证"入池即保证能成功"，
    /// 避免把必然失败的交易广播给全网。
    fn validate_defi_payload(&self, transaction: &Transaction) -> CoreResult<()> {
        let payload = decode_defi_payload(transaction)?;
        payload
            .validate()
            .map_err(|error| CoreError::DefiPayloadInvalid {
                tx_id: transaction.id.clone(),
                reason: error.to_string(),
            })?;

        // 除清算外，载荷 owner 必须与发送方一致，防止代他人操作仓位。
        // 清理由第三方清算人对借款人仓位发起，故豁免该校验。
        if payload.is_self_operated() && payload.owner != transaction.from {
            return Err(CoreError::DefiOwnerMismatch {
                owner: payload.owner.clone(),
                sender: transaction.from.clone(),
            });
        }

        let mut probe = self.lending_pool.clone();
        let result = match payload.action {
            DefiAction::DepositCollateral => probe
                .deposit_collateral(&payload.owner, payload.amount)
                .map(|_| ()),
            DefiAction::Borrow => probe
                .borrow(&payload.owner, payload.amount, transaction.timestamp)
                .map(|_| ()),
            DefiAction::Repay => probe
                .repay(&payload.owner, payload.amount, transaction.timestamp)
                .map(|_| ()),
            DefiAction::WithdrawCollateral => probe
                .withdraw_collateral(&payload.owner, payload.amount, transaction.timestamp)
                .map(|_| ()),
            DefiAction::Liquidate => probe
                .liquidate(&payload.owner, payload.amount, transaction.timestamp)
                .map(|_| ()),
        };

        result.map_err(|error| CoreError::DefiExecutionFailed {
            tx_id: transaction.id.clone(),
            reason: error.to_string(),
        })
    }

    /// 在区块确认后推进链上业务状态（合约状态/事件与 DeFi 借贷池）。
    fn apply_state_transitions(&mut self, transactions: &[Transaction]) -> CoreResult<()> {
        for transaction in transactions {
            match transaction.kind {
                TransactionKind::ContractDeploy | TransactionKind::ContractCall => {
                    self.apply_contract_transition(transaction)?;
                }
                TransactionKind::DefiAction => self.apply_defi_transition(transaction)?,
                _ => {}
            }
        }

        Ok(())
    }

    /// 推进单个合约交易的状态与事件。
    fn apply_contract_transition(&mut self, transaction: &Transaction) -> CoreResult<()> {
        let Some(source) = decode_contract_source(transaction)? else {
            return Ok(());
        };

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

        Ok(())
    }

    /// 推进单个 DeFi 交易对应的借贷池状态。
    fn apply_defi_transition(&mut self, transaction: &Transaction) -> CoreResult<()> {
        let payload = decode_defi_payload(transaction)?;
        payload
            .validate()
            .map_err(|error| CoreError::DefiPayloadInvalid {
                tx_id: transaction.id.clone(),
                reason: error.to_string(),
            })?;

        let now_ts = transaction.timestamp;
        let result = match payload.action {
            DefiAction::DepositCollateral => self
                .lending_pool
                .deposit_collateral(&payload.owner, payload.amount)
                .map(|_| ()),
            DefiAction::Borrow => self
                .lending_pool
                .borrow(&payload.owner, payload.amount, now_ts)
                .map(|_| ()),
            DefiAction::Repay => self
                .lending_pool
                .repay(&payload.owner, payload.amount, now_ts)
                .map(|_| ()),
            DefiAction::WithdrawCollateral => self
                .lending_pool
                .withdraw_collateral(&payload.owner, payload.amount, now_ts)
                .map(|_| ()),
            DefiAction::Liquidate => self
                .lending_pool
                .liquidate(&payload.owner, payload.amount, now_ts)
                .map(|_| ()),
        };

        result.map_err(|error| CoreError::DefiExecutionFailed {
            tx_id: transaction.id.clone(),
            reason: error.to_string(),
        })
    }
}

/// 判断交易是否属于"内部动作"。
///
/// 这类交易的 `amount` 表达的是业务数量（如抵押品数量），而非账户间转账金额，
/// 因此不应影响账户余额。DeFi 动作目前是唯一的内部动作类型。
fn is_internal_action(transaction: &Transaction) -> bool {
    matches!(transaction.kind, TransactionKind::DefiAction)
}

/// 解码合约源码；非合约交易或空载荷返回 `None`。
fn decode_contract_source(transaction: &Transaction) -> CoreResult<Option<&str>> {
    let Some(raw_payload) = transaction.payload.as_ref() else {
        return Ok(None);
    };
    if raw_payload.is_empty() {
        return Ok(None);
    }

    let source = std::str::from_utf8(raw_payload).map_err(|error| {
        CoreError::ContractPayloadEncodingInvalid {
            tx_id: transaction.id.clone(),
            reason: error.to_string(),
        }
    })?;

    if source.trim().is_empty() {
        return Ok(None);
    }

    Ok(Some(source))
}

/// 解码 DeFi 载荷，缺失或非法时返回可读错误。
fn decode_defi_payload(transaction: &Transaction) -> CoreResult<DefiPayload> {
    let raw_payload =
        transaction
            .payload
            .as_ref()
            .ok_or_else(|| CoreError::DefiPayloadInvalid {
                tx_id: transaction.id.clone(),
                reason: "缺少 DeFi 载荷".to_string(),
            })?;

    DefiPayload::decode(raw_payload).map_err(|error| CoreError::DefiPayloadInvalid {
        tx_id: transaction.id.clone(),
        reason: error.to_string(),
    })
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

    /// 验证可查询最新区块难度与下一块期望难度。
    #[test]
    fn difficulty_query_methods_should_work() {
        let mut blockchain = Blockchain::new(2, 50);
        assert_eq!(
            blockchain
                .next_block_expected_difficulty()
                .expect("应可计算下一块难度"),
            2
        );
        assert_eq!(
            blockchain
                .latest_block_difficulty()
                .expect("应可读取最新区块难度"),
            0
        );

        let block = blockchain
            .mine_pending_transactions("miner-q")
            .expect("挖矿应成功");
        assert_eq!(block.difficulty, 2);
        assert_eq!(
            blockchain
                .latest_block_difficulty()
                .expect("应可读取最新区块难度"),
            2
        );
    }

    /// 构造一笔已签名的 DeFi 交易，便于复用。
    fn signed_defi_tx(
        wallet: &rustchain_crypto::wallet::Wallet,
        key_pair: &rustchain_crypto::wallet::WalletKeyPair,
        action: DefiAction,
        owner: &str,
        amount: u64,
        nonce: u64,
        timestamp: i64,
    ) -> Transaction {
        let payload = DefiPayload::new(action, owner, amount)
            .encode()
            .expect("载荷编码应成功");
        let mut tx = Transaction::new_with_kind(
            TransactionKind::DefiAction,
            wallet.address.clone(),
            "defi-lending-pool",
            amount,
            nonce,
            Some(payload),
        );
        // 用固定时间戳替代构造时的当前时间，保证计息可预测。
        tx.timestamp = timestamp;
        tx.sign_with_private_key(&key_pair.private_key, &key_pair.public_key)
            .expect("交易签名应当成功");
        tx
    }

    /// 验证 DeFi 抵押交易在出块前不改变借贷池，出块后才推进状态。
    #[test]
    fn defi_deposit_should_apply_only_after_mining() {
        let (wallet, key_pair) = create_wallet("defi-pass").expect("创建钱包应当成功");
        let mut blockchain = Blockchain::new(1, 50);
        blockchain
            .mine_pending_transactions(wallet.address.clone())
            .expect("首次挖矿应成功");

        let tx = signed_defi_tx(
            &wallet,
            &key_pair,
            DefiAction::DepositCollateral,
            &wallet.address,
            200,
            0,
            0,
        );
        blockchain.add_transaction(tx).expect("抵押交易应入池");

        // 入池阶段只做试执行校验，不得改动链上借贷池。
        assert!(blockchain.lending_pool.positions.is_empty());
        assert_eq!(blockchain.lending_pool.total_collateral, 0);

        blockchain
            .mine_pending_transactions("miner-2")
            .expect("出块应成功");

        let position = blockchain
            .lending_pool
            .positions
            .get(&wallet.address)
            .expect("应存在仓位");
        assert_eq!(position.collateral_amount, 200);
        assert_eq!(blockchain.lending_pool.total_collateral, 200);
    }

    /// 验证抵押后可借款，且借款后抵押率被正确记录。
    #[test]
    fn defi_borrow_after_deposit_should_work() {
        let (wallet, key_pair) = create_wallet("defi-pass").expect("创建钱包应当成功");
        let mut blockchain = Blockchain::new(1, 50);
        blockchain
            .mine_pending_transactions(wallet.address.clone())
            .expect("首次挖矿应成功");

        let deposit = signed_defi_tx(
            &wallet,
            &key_pair,
            DefiAction::DepositCollateral,
            &wallet.address,
            300,
            0,
            0,
        );
        blockchain.add_transaction(deposit).expect("抵押应入池");
        blockchain
            .mine_pending_transactions("miner-2")
            .expect("出块应成功");

        let borrow = signed_defi_tx(
            &wallet,
            &key_pair,
            DefiAction::Borrow,
            &wallet.address,
            100,
            1,
            1,
        );
        blockchain.add_transaction(borrow).expect("借款应入池");
        blockchain
            .mine_pending_transactions("miner-3")
            .expect("出块应成功");

        let position = blockchain
            .lending_pool
            .positions
            .get(&wallet.address)
            .expect("应存在仓位");
        assert_eq!(position.debt_amount, 100);
        assert_eq!(position.collateral_ratio_bps, 30_000);
    }

    /// 验证超额借款会在入池阶段就被拒绝，不会进入交易池。
    #[test]
    fn defi_over_borrow_should_be_rejected_at_mempool() {
        let (wallet, key_pair) = create_wallet("defi-pass").expect("创建钱包应当成功");
        let mut blockchain = Blockchain::new(1, 50);
        blockchain
            .mine_pending_transactions(wallet.address.clone())
            .expect("首次挖矿应成功");

        let deposit = signed_defi_tx(
            &wallet,
            &key_pair,
            DefiAction::DepositCollateral,
            &wallet.address,
            100,
            0,
            0,
        );
        blockchain.add_transaction(deposit).expect("抵押应入池");
        blockchain
            .mine_pending_transactions("miner-2")
            .expect("出块应成功");

        // 抵押 100 最多借出约 66，借 100 必然触发抵押率不足。
        let borrow = signed_defi_tx(
            &wallet,
            &key_pair,
            DefiAction::Borrow,
            &wallet.address,
            100,
            1,
            1,
        );
        let result = blockchain.add_transaction(borrow);

        assert!(
            matches!(result, Err(CoreError::DefiExecutionFailed { .. })),
            "超额借款应被拒绝，实际: {result:?}"
        );
        assert_eq!(blockchain.pending_transactions.len(), 0);
    }

    /// 验证非清算动作不允许代他人操作仓位。
    #[test]
    fn defi_action_on_other_owner_should_be_rejected() {
        let (alice_wallet, alice_key) = create_wallet("alice-pass").expect("创建钱包应当成功");
        let (bob_wallet, _) = create_wallet("bob-pass").expect("创建钱包应当成功");
        let mut blockchain = Blockchain::new(1, 50);
        blockchain
            .mine_pending_transactions(alice_wallet.address.clone())
            .expect("首次挖矿应成功");

        // alice 签名，但载荷 owner 写成 bob。
        let tx = signed_defi_tx(
            &alice_wallet,
            &alice_key,
            DefiAction::DepositCollateral,
            &bob_wallet.address,
            50,
            0,
            0,
        );
        let result = blockchain.add_transaction(tx);

        assert!(
            matches!(result, Err(CoreError::DefiOwnerMismatch { .. })),
            "代他人操作应被拒绝，实际: {result:?}"
        );
    }

    /// 验证缺失载荷的 DeFi 交易会被拒绝。
    #[test]
    fn defi_transaction_without_payload_should_be_rejected() {
        let (wallet, key_pair) = create_wallet("defi-pass").expect("创建钱包应当成功");
        let mut blockchain = Blockchain::new(1, 50);
        blockchain
            .mine_pending_transactions(wallet.address.clone())
            .expect("首次挖矿应成功");

        let mut tx = Transaction::new_with_kind(
            TransactionKind::DefiAction,
            wallet.address.clone(),
            "defi-lending-pool",
            10,
            0,
            None,
        );
        tx.sign_with_private_key(&key_pair.private_key, &key_pair.public_key)
            .expect("交易签名应当成功");

        let result = blockchain.add_transaction(tx);
        assert!(
            matches!(result, Err(CoreError::DefiPayloadInvalid { .. })),
            "缺失载荷应被拒绝，实际: {result:?}"
        );
    }
}
