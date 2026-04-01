use sha2::{Digest, Sha256};

/// 计算字节数组的 SHA-256 十六进制字符串。
pub fn sha256_hex(data: &[u8]) -> String {
    let digest = Sha256::digest(data);
    hex::encode(digest)
}

/// 按顺序拼接多段字节并统一计算哈希。
pub fn sha256_hex_parts(parts: &[&[u8]]) -> String {
    let mut hasher = Sha256::new();

    for part in parts {
        hasher.update(part);
    }

    hex::encode(hasher.finalize())
}
