pub trait StateStore {
    fn get_balance(&self, account: &str) -> anyhow::Result<Option<u64>>;
    fn set_balance(&self, account: &str, balance: u64) -> anyhow::Result<()>;
}
