pub trait HistoryStore {
    fn get_block(&self, block_hash: &str) -> anyhow::Result<Option<Vec<u8>>>;
    fn put_block(&self, block_hash: &str, encoded: &[u8]) -> anyhow::Result<()>;
}
