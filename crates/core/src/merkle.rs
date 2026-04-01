use crate::hash::sha256_hex_parts;

/// 根据叶子节点哈希列表计算 Merkle Root。
pub fn calculate_merkle_root(leaves: &[String]) -> String {
    if leaves.is_empty() {
        return sha256_hex_parts(&[b"empty-merkle-root"]);
    }

    let mut level = leaves.to_vec();

    while level.len() > 1 {
        let mut next_level = Vec::with_capacity(level.len().div_ceil(2));

        for chunk in level.chunks(2) {
            let left = chunk[0].as_bytes();
            let right = chunk.get(1).unwrap_or(&chunk[0]).as_bytes();
            next_level.push(sha256_hex_parts(&[left, right]));
        }

        level = next_level;
    }

    level.remove(0)
}
