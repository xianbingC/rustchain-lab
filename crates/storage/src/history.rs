use crate::{
    error::{StorageError, StorageResult},
    StorageResult as Result,
};
use rusty_leveldb::{Options, DB};
use std::{
    collections::HashMap,
    path::Path,
    sync::{Mutex, MutexGuard},
};

/// 历史数据存储抽象，负责区块与交易历史读写。
pub trait HistoryStore {
    /// 查询区块历史数据。
    fn get_block(&self, block_hash: &str) -> Result<Option<Vec<u8>>>;
    /// 写入区块历史数据。
    fn put_block(&self, block_hash: &str, encoded: &[u8]) -> Result<()>;
    /// 删除区块历史数据。
    fn delete_block(&self, block_hash: &str) -> Result<()>;
    /// 查询交易历史数据。
    fn get_transaction(&self, tx_id: &str) -> Result<Option<Vec<u8>>>;
    /// 写入交易历史数据。
    fn put_transaction(&self, tx_id: &str, encoded: &[u8]) -> Result<()>;
    /// 删除交易历史数据。
    fn delete_transaction(&self, tx_id: &str) -> Result<()>;
}

/// 内存历史存储实现，适用于测试与无 LevelDB 场景。
#[derive(Default)]
pub struct InMemoryHistoryStore {
    blocks: Mutex<HashMap<String, Vec<u8>>>,
    transactions: Mutex<HashMap<String, Vec<u8>>>,
}

impl InMemoryHistoryStore {
    /// 创建内存历史存储实例。
    pub fn new() -> Self {
        Self::default()
    }

    /// 获取区块历史锁。
    fn blocks_lock(&self) -> Result<MutexGuard<'_, HashMap<String, Vec<u8>>>> {
        self.blocks.lock().map_err(|_| StorageError::PoisonedLock)
    }

    /// 获取交易历史锁。
    fn transactions_lock(&self) -> Result<MutexGuard<'_, HashMap<String, Vec<u8>>>> {
        self.transactions
            .lock()
            .map_err(|_| StorageError::PoisonedLock)
    }
}

impl HistoryStore for InMemoryHistoryStore {
    fn get_block(&self, block_hash: &str) -> Result<Option<Vec<u8>>> {
        let blocks = self.blocks_lock()?;
        Ok(blocks.get(block_hash).cloned())
    }

    fn put_block(&self, block_hash: &str, encoded: &[u8]) -> Result<()> {
        let mut blocks = self.blocks_lock()?;
        blocks.insert(block_hash.to_string(), encoded.to_vec());
        Ok(())
    }

    fn delete_block(&self, block_hash: &str) -> Result<()> {
        let mut blocks = self.blocks_lock()?;
        blocks.remove(block_hash);
        Ok(())
    }

    fn get_transaction(&self, tx_id: &str) -> Result<Option<Vec<u8>>> {
        let transactions = self.transactions_lock()?;
        Ok(transactions.get(tx_id).cloned())
    }

    fn put_transaction(&self, tx_id: &str, encoded: &[u8]) -> Result<()> {
        let mut transactions = self.transactions_lock()?;
        transactions.insert(tx_id.to_string(), encoded.to_vec());
        Ok(())
    }

    fn delete_transaction(&self, tx_id: &str) -> Result<()> {
        let mut transactions = self.transactions_lock()?;
        transactions.remove(tx_id);
        Ok(())
    }
}

/// LevelDB 历史存储实现（基于 `rusty-leveldb`）。
pub struct LevelDbHistoryStore {
    db: Mutex<DB>,
}

impl LevelDbHistoryStore {
    /// 打开或创建 LevelDB 历史库。
    pub fn open(path: impl AsRef<Path>) -> StorageResult<Self> {
        let mut options = Options::default();
        options.create_if_missing = true;

        let db =
            DB::open(path, options).map_err(|error| StorageError::LevelDb(format!("{error:?}")))?;
        Ok(Self { db: Mutex::new(db) })
    }

    /// 生成区块历史键。
    fn block_key(block_hash: &str) -> String {
        format!("history:block:{block_hash}")
    }

    /// 生成交易历史键。
    fn tx_key(tx_id: &str) -> String {
        format!("history:tx:{tx_id}")
    }

    /// 从 LevelDB 读取键值。
    fn get_raw(&self, key: &[u8]) -> Result<Option<Vec<u8>>> {
        let mut db = self.db.lock().map_err(|_| StorageError::PoisonedLock)?;
        Ok(db.get(key).map(|value| value.to_vec()))
    }

    /// 向 LevelDB 写入键值并立即 flush。
    fn put_raw(&self, key: &[u8], value: &[u8]) -> Result<()> {
        let mut db = self.db.lock().map_err(|_| StorageError::PoisonedLock)?;
        db.put(key, value)
            .map_err(|error| StorageError::LevelDb(format!("{error:?}")))?;
        db.flush()
            .map_err(|error| StorageError::LevelDb(format!("{error:?}")))
    }

    /// 删除 LevelDB 键并立即 flush。
    fn delete_raw(&self, key: &[u8]) -> Result<()> {
        let mut db = self.db.lock().map_err(|_| StorageError::PoisonedLock)?;
        db.delete(key)
            .map_err(|error| StorageError::LevelDb(format!("{error:?}")))?;
        db.flush()
            .map_err(|error| StorageError::LevelDb(format!("{error:?}")))
    }
}

impl HistoryStore for LevelDbHistoryStore {
    fn get_block(&self, block_hash: &str) -> Result<Option<Vec<u8>>> {
        let key = Self::block_key(block_hash);
        self.get_raw(key.as_bytes())
    }

    fn put_block(&self, block_hash: &str, encoded: &[u8]) -> Result<()> {
        let key = Self::block_key(block_hash);
        self.put_raw(key.as_bytes(), encoded)
    }

    fn delete_block(&self, block_hash: &str) -> Result<()> {
        let key = Self::block_key(block_hash);
        self.delete_raw(key.as_bytes())
    }

    fn get_transaction(&self, tx_id: &str) -> Result<Option<Vec<u8>>> {
        let key = Self::tx_key(tx_id);
        self.get_raw(key.as_bytes())
    }

    fn put_transaction(&self, tx_id: &str, encoded: &[u8]) -> Result<()> {
        let key = Self::tx_key(tx_id);
        self.put_raw(key.as_bytes(), encoded)
    }

    fn delete_transaction(&self, tx_id: &str) -> Result<()> {
        let key = Self::tx_key(tx_id);
        self.delete_raw(key.as_bytes())
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use tempfile::TempDir;

    /// 验证内存历史存储支持区块与交易读写。
    #[test]
    fn in_memory_history_roundtrip_should_work() {
        let store = InMemoryHistoryStore::new();
        store
            .put_block("block-a", b"block-bytes")
            .expect("写入区块应成功");
        store
            .put_transaction("tx-a", b"tx-bytes")
            .expect("写入交易应成功");

        assert_eq!(
            store.get_block("block-a").expect("读取区块应成功"),
            Some(b"block-bytes".to_vec())
        );
        assert_eq!(
            store.get_transaction("tx-a").expect("读取交易应成功"),
            Some(b"tx-bytes".to_vec())
        );
    }

    /// 验证区块历史写入与读取。
    #[test]
    fn block_history_roundtrip_should_work() {
        let dir = TempDir::new().expect("创建临时目录应当成功");
        let db_path = dir.path().join("history-db");
        let store = LevelDbHistoryStore::open(&db_path).expect("打开 LevelDB 应当成功");

        store
            .put_block("block-hash-1", b"block-bytes")
            .expect("写入区块历史应当成功");
        let block = store
            .get_block("block-hash-1")
            .expect("读取区块历史应当成功");
        assert_eq!(block, Some(b"block-bytes".to_vec()));
    }

    /// 验证交易历史写入与删除。
    #[test]
    fn transaction_history_delete_should_work() {
        let dir = TempDir::new().expect("创建临时目录应当成功");
        let db_path = dir.path().join("history-db");
        let store = LevelDbHistoryStore::open(&db_path).expect("打开 LevelDB 应当成功");

        store
            .put_transaction("tx-1", b"tx-bytes")
            .expect("写入交易历史应当成功");
        let tx = store.get_transaction("tx-1").expect("读取交易历史应当成功");
        assert_eq!(tx, Some(b"tx-bytes".to_vec()));

        store
            .delete_transaction("tx-1")
            .expect("删除交易历史应当成功");
        let deleted = store.get_transaction("tx-1").expect("读取交易历史应当成功");
        assert_eq!(deleted, None);
    }
}
