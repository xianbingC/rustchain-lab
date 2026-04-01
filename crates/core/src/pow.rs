/// 判断哈希值是否满足当前 PoW 难度。
pub fn meets_difficulty(hash: &str, difficulty: u32) -> bool {
    let prefix = "0".repeat(difficulty as usize);
    hash.starts_with(&prefix)
}
