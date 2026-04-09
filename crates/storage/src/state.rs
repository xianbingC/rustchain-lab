use crate::{
    error::StorageError,
    StorageResult as Result,
};
use std::{
    collections::HashMap,
    sync::{Mutex, MutexGuard},
};

/// 链状态存储抽象，负责账户与合约状态读写。
pub trait StateStore {
    /// 查询账户余额。
    fn get_balance(&self, account: &str) -> Result<Option<u64>>;
    /// 设置账户余额。
    fn set_balance(&self, account: &str, balance: u64) -> Result<()>;
    /// 删除账户状态。
    fn delete_account(&self, account: &str) -> Result<()>;
    /// 查询合约状态字段。
    fn get_contract_state(&self, contract: &str, field: &str) -> Result<Option<Vec<u8>>>;
    /// 设置合约状态字段。
    fn set_contract_state(&self, contract: &str, field: &str, value: &[u8]) -> Result<()>;
    /// 删除合约状态字段。
    fn delete_contract_state(&self, contract: &str, field: &str) -> Result<()>;
}

/// 内存状态存储实现，主要用于无 RocksDB 环境的开发和测试。
pub struct InMemoryStateStore {
    balances: Mutex<HashMap<String, u64>>,
    contract_states: Mutex<HashMap<String, Vec<u8>>>,
}

impl Default for InMemoryStateStore {
    fn default() -> Self {
        Self::new()
    }
}

impl InMemoryStateStore {
    /// 创建内存状态存储实例。
    pub fn new() -> Self {
        Self {
            balances: Mutex::new(HashMap::new()),
            contract_states: Mutex::new(HashMap::new()),
        }
    }

    /// 生成合约状态 map 键。
    fn contract_key(contract: &str, field: &str) -> String {
        format!("{contract}:{field}")
    }

    /// 获取余额锁。
    fn balances_lock(&self) -> Result<MutexGuard<'_, HashMap<String, u64>>> {
        self.balances.lock().map_err(|_| StorageError::PoisonedLock)
    }

    /// 获取合约状态锁。
    fn contract_states_lock(&self) -> Result<MutexGuard<'_, HashMap<String, Vec<u8>>>> {
        self.contract_states
            .lock()
            .map_err(|_| StorageError::PoisonedLock)
    }
}

impl StateStore for InMemoryStateStore {
    fn get_balance(&self, account: &str) -> Result<Option<u64>> {
        let balances = self.balances_lock()?;
        Ok(balances.get(account).copied())
    }

    fn set_balance(&self, account: &str, balance: u64) -> Result<()> {
        let mut balances = self.balances_lock()?;
        balances.insert(account.to_string(), balance);
        Ok(())
    }

    fn delete_account(&self, account: &str) -> Result<()> {
        let mut balances = self.balances_lock()?;
        balances.remove(account);
        Ok(())
    }

    fn get_contract_state(&self, contract: &str, field: &str) -> Result<Option<Vec<u8>>> {
        let contract_states = self.contract_states_lock()?;
        let key = Self::contract_key(contract, field);
        Ok(contract_states.get(&key).cloned())
    }

    fn set_contract_state(&self, contract: &str, field: &str, value: &[u8]) -> Result<()> {
        let mut contract_states = self.contract_states_lock()?;
        let key = Self::contract_key(contract, field);
        contract_states.insert(key, value.to_vec());
        Ok(())
    }

    fn delete_contract_state(&self, contract: &str, field: &str) -> Result<()> {
        let mut contract_states = self.contract_states_lock()?;
        let key = Self::contract_key(contract, field);
        contract_states.remove(&key);
        Ok(())
    }
}

/// RocksDB 状态存储实现。
#[cfg(feature = "rocksdb-backend")]
pub struct RocksDbStateStore {
    db: rocksdb::DB,
}

#[cfg(feature = "rocksdb-backend")]
impl RocksDbStateStore {
    /// 打开或创建 RocksDB 状态库。
    pub fn open(path: impl AsRef<std::path::Path>) -> StorageResult<Self> {
        let mut options = rocksdb::Options::default();
        options.create_if_missing(true);

        let db = rocksdb::DB::open(&options, path)
            .map_err(|error| StorageError::RocksDb(error.to_string()))?;
        Ok(Self { db })
    }

    /// 生成账户余额键。
    fn balance_key(account: &str) -> String {
        format!("state:account:balance:{account}")
    }

    /// 生成合约状态键。
    fn contract_key(contract: &str, field: &str) -> String {
        format!("state:contract:{contract}:{field}")
    }
}

#[cfg(feature = "rocksdb-backend")]
impl StateStore for RocksDbStateStore {
    fn get_balance(&self, account: &str) -> Result<Option<u64>> {
        let key = Self::balance_key(account);
        let value = self
            .db
            .get(key.as_bytes())
            .map_err(|error| StorageError::RocksDb(error.to_string()))?;

        match value {
            None => Ok(None),
            Some(raw) => {
                if raw.len() != 8 {
                    return Err(StorageError::Codec(format!(
                        "账户余额字节长度非法: {}",
                        raw.len()
                    )));
                }
                let mut bytes = [0u8; 8];
                bytes.copy_from_slice(&raw);
                Ok(Some(u64::from_le_bytes(bytes)))
            }
        }
    }

    fn set_balance(&self, account: &str, balance: u64) -> Result<()> {
        let key = Self::balance_key(account);
        self.db
            .put(key.as_bytes(), balance.to_le_bytes())
            .map_err(|error| StorageError::RocksDb(error.to_string()))
    }

    fn delete_account(&self, account: &str) -> Result<()> {
        let key = Self::balance_key(account);
        self.db
            .delete(key.as_bytes())
            .map_err(|error| StorageError::RocksDb(error.to_string()))
    }

    fn get_contract_state(&self, contract: &str, field: &str) -> Result<Option<Vec<u8>>> {
        let key = Self::contract_key(contract, field);
        self.db
            .get(key.as_bytes())
            .map_err(|error| StorageError::RocksDb(error.to_string()))
    }

    fn set_contract_state(&self, contract: &str, field: &str, value: &[u8]) -> Result<()> {
        let key = Self::contract_key(contract, field);
        self.db
            .put(key.as_bytes(), value)
            .map_err(|error| StorageError::RocksDb(error.to_string()))
    }

    fn delete_contract_state(&self, contract: &str, field: &str) -> Result<()> {
        let key = Self::contract_key(contract, field);
        self.db
            .delete(key.as_bytes())
            .map_err(|error| StorageError::RocksDb(error.to_string()))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// 验证内存账户余额写入与读取。
    #[test]
    fn in_memory_account_balance_roundtrip_should_work() {
        let store = InMemoryStateStore::new();

        store
            .set_balance("alice", 88)
            .expect("写入余额应当成功");
        let balance = store.get_balance("alice").expect("读取余额应当成功");

        assert_eq!(balance, Some(88));
    }

    /// 验证内存合约状态写入、读取与删除。
    #[test]
    fn in_memory_contract_state_roundtrip_should_work() {
        let store = InMemoryStateStore::new();

        store
            .set_contract_state("loan-v1", "interest_bps", b"350")
            .expect("写入合约状态应当成功");
        let value = store
            .get_contract_state("loan-v1", "interest_bps")
            .expect("读取合约状态应当成功");
        assert_eq!(value, Some(b"350".to_vec()));

        store
            .delete_contract_state("loan-v1", "interest_bps")
            .expect("删除合约状态应当成功");
        let deleted = store
            .get_contract_state("loan-v1", "interest_bps")
            .expect("读取合约状态应当成功");
        assert_eq!(deleted, None);
    }

    /// 验证 RocksDB 功能在启用特性时可用。
    #[cfg(feature = "rocksdb-backend")]
    #[test]
    fn rocksdb_balance_roundtrip_should_work() {
        let dir = tempfile::TempDir::new().expect("创建临时目录应当成功");
        let store = RocksDbStateStore::open(dir.path()).expect("打开 RocksDB 应当成功");

        store
            .set_balance("bob", 99)
            .expect("写入 RocksDB 余额应当成功");
        let balance = store.get_balance("bob").expect("读取 RocksDB 余额应当成功");
        assert_eq!(balance, Some(99));
    }
}
