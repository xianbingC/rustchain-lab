use crate::{
    error::{CryptoError, CryptoResult},
    signature::{sign_message, verify_message},
};
use ed25519_dalek::{SigningKey, VerifyingKey};
use rand::rngs::OsRng;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{fs, path::Path};

/// 钱包结构，保存可公开数据和加密私钥。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Wallet {
    /// 地址（由公钥派生）。
    pub address: String,
    /// 十六进制公钥。
    pub public_key: String,
    /// 十六进制加密私钥。
    pub encrypted_private_key: String,
    /// 十六进制盐值。
    pub kdf_salt: String,
    /// 私钥校验摘要，用于检测密码错误或数据损坏。
    pub private_key_checksum: String,
}

/// 钱包创建时返回的密钥对。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WalletKeyPair {
    /// 地址（与钱包地址一致）。
    pub address: String,
    /// 十六进制公钥。
    pub public_key: String,
    /// 十六进制私钥（仅在创建时返回，业务层应谨慎处理）。
    pub private_key: String,
}

/// 生成新钱包，并使用密码加密私钥。
pub fn create_wallet(password: &str) -> CryptoResult<(Wallet, WalletKeyPair)> {
    if password.is_empty() {
        return Err(CryptoError::EmptyPassword);
    }

    let signing_key = SigningKey::generate(&mut OsRng);
    let verifying_key: VerifyingKey = signing_key.verifying_key();

    let private_key = hex::encode(signing_key.to_bytes());
    let public_key = hex::encode(verifying_key.to_bytes());
    let address = derive_address(&verifying_key);

    let salt_bytes = SigningKey::generate(&mut OsRng).to_bytes();
    let salt_hex = hex::encode(salt_bytes);
    let encrypted_private_key = encrypt_private_key(&private_key, password, &salt_hex)?;

    let wallet = Wallet {
        address: address.clone(),
        public_key: public_key.clone(),
        encrypted_private_key,
        kdf_salt: salt_hex,
        private_key_checksum: checksum_hex(&private_key),
    };

    let key_pair = WalletKeyPair {
        address,
        public_key,
        private_key,
    };

    Ok((wallet, key_pair))
}

/// 使用已有私钥导入钱包，并使用密码加密存储。
pub fn create_wallet_from_private_key(
    private_key_hex: &str,
    password: &str,
) -> CryptoResult<(Wallet, WalletKeyPair)> {
    if password.is_empty() {
        return Err(CryptoError::EmptyPassword);
    }

    let private_key_bytes = decode_hex(private_key_hex)?;
    if private_key_bytes.len() != 32 {
        return Err(CryptoError::InvalidKeyLength {
            expected: 32,
            actual: private_key_bytes.len(),
        });
    }
    let mut private_key_array = [0u8; 32];
    private_key_array.copy_from_slice(&private_key_bytes);

    let signing_key = SigningKey::from_bytes(&private_key_array);
    let verifying_key: VerifyingKey = signing_key.verifying_key();
    let private_key = hex::encode(signing_key.to_bytes());
    let public_key = hex::encode(verifying_key.to_bytes());
    let address = derive_address(&verifying_key);

    let salt_bytes = SigningKey::generate(&mut OsRng).to_bytes();
    let salt_hex = hex::encode(salt_bytes);
    let encrypted_private_key = encrypt_private_key(&private_key, password, &salt_hex)?;

    let wallet = Wallet {
        address: address.clone(),
        public_key: public_key.clone(),
        encrypted_private_key,
        kdf_salt: salt_hex,
        private_key_checksum: checksum_hex(&private_key),
    };
    validate_wallet_integrity(&wallet)?;

    let key_pair = WalletKeyPair {
        address,
        public_key,
        private_key,
    };
    Ok((wallet, key_pair))
}

/// 将钱包导出为 JSON 字符串，便于备份和迁移。
pub fn export_wallet_to_json(wallet: &Wallet) -> CryptoResult<String> {
    validate_wallet_integrity(wallet)?;
    serde_json::to_string_pretty(wallet)
        .map_err(|error| CryptoError::WalletSerde(error.to_string()))
}

/// 从 JSON 字符串导入钱包并进行完整性校验。
pub fn import_wallet_from_json(raw: &str) -> CryptoResult<Wallet> {
    let wallet = serde_json::from_str::<Wallet>(raw)
        .map_err(|error| CryptoError::WalletSerde(error.to_string()))?;
    validate_wallet_integrity(&wallet)?;
    Ok(wallet)
}

/// 将钱包写入备份文件（JSON 格式）。
pub fn backup_wallet_to_file(wallet: &Wallet, path: impl AsRef<Path>) -> CryptoResult<()> {
    let json = export_wallet_to_json(wallet)?;
    fs::write(path, json).map_err(|error| CryptoError::WalletIo(error.to_string()))
}

/// 从备份文件恢复钱包（JSON 格式）。
pub fn restore_wallet_from_file(path: impl AsRef<Path>) -> CryptoResult<Wallet> {
    let raw = fs::read_to_string(path).map_err(|error| CryptoError::WalletIo(error.to_string()))?;
    import_wallet_from_json(&raw)
}

impl Wallet {
    /// 使用密码解密私钥（十六进制）。
    pub fn decrypt_private_key(&self, password: &str) -> CryptoResult<String> {
        let private_key =
            decrypt_private_key(&self.encrypted_private_key, password, &self.kdf_salt)?;
        if checksum_hex(&private_key) != self.private_key_checksum {
            return Err(CryptoError::WalletDecryptFailed);
        }

        Ok(private_key)
    }

    /// 使用钱包签名消息。
    pub fn sign_message(&self, message: &[u8], password: &str) -> CryptoResult<String> {
        let private_key = self.decrypt_private_key(password)?;
        sign_message(message, &private_key)
    }

    /// 使用钱包公钥验证签名。
    pub fn verify_message(&self, message: &[u8], signature_hex: &str) -> CryptoResult<bool> {
        verify_message(message, signature_hex, &self.public_key)
    }
}

/// 使用密码和盐值加密私钥（原型阶段使用异或流加密）。
fn encrypt_private_key(
    private_key_hex: &str,
    password: &str,
    salt_hex: &str,
) -> CryptoResult<String> {
    if password.is_empty() {
        return Err(CryptoError::EmptyPassword);
    }

    let private_key = decode_hex(private_key_hex)?;
    let key_stream = derive_key_stream(password, salt_hex, private_key.len());
    let cipher = xor_bytes(&private_key, &key_stream);

    Ok(hex::encode(cipher))
}

/// 解密私钥（原型阶段对应异或流解密）。
fn decrypt_private_key(
    encrypted_private_key_hex: &str,
    password: &str,
    salt_hex: &str,
) -> CryptoResult<String> {
    if password.is_empty() {
        return Err(CryptoError::EmptyPassword);
    }

    let cipher = decode_hex(encrypted_private_key_hex)?;
    let key_stream = derive_key_stream(password, salt_hex, cipher.len());
    let plain = xor_bytes(&cipher, &key_stream);

    if plain.len() != 32 {
        return Err(CryptoError::WalletDecryptFailed);
    }

    Ok(hex::encode(plain))
}

/// 派生地址：`sha256(pubkey)` 前 20 字节。
fn derive_address(verifying_key: &VerifyingKey) -> String {
    let digest = Sha256::digest(verifying_key.to_bytes());
    format!("0x{}", hex::encode(&digest[..20]))
}

/// 根据十六进制公钥派生地址，便于外部模块进行签名地址一致性校验。
pub fn derive_address_from_public_key(public_key_hex: &str) -> CryptoResult<String> {
    let public_key = decode_hex(public_key_hex)?;
    if public_key.len() != 32 {
        return Err(CryptoError::InvalidKeyLength {
            expected: 32,
            actual: public_key.len(),
        });
    }

    let mut public_key_bytes = [0u8; 32];
    public_key_bytes.copy_from_slice(&public_key);
    let verifying_key = VerifyingKey::from_bytes(&public_key_bytes)
        .map_err(|_| CryptoError::InvalidHex("公钥内容不合法".to_string()))?;

    Ok(derive_address(&verifying_key))
}

/// 按长度生成密钥流。
fn derive_key_stream(password: &str, salt_hex: &str, len: usize) -> Vec<u8> {
    let mut stream = Vec::with_capacity(len);
    let mut counter: u64 = 0;

    while stream.len() < len {
        let mut hasher = Sha256::new();
        hasher.update(password.as_bytes());
        hasher.update(salt_hex.as_bytes());
        hasher.update(counter.to_le_bytes());
        let block = hasher.finalize();
        stream.extend_from_slice(&block);
        counter = counter.saturating_add(1);
    }

    stream.truncate(len);
    stream
}

/// 对两段字节执行异或。
fn xor_bytes(left: &[u8], right: &[u8]) -> Vec<u8> {
    left.iter().zip(right.iter()).map(|(a, b)| a ^ b).collect()
}

/// 十六进制解码。
fn decode_hex(value: &str) -> CryptoResult<Vec<u8>> {
    hex::decode(value).map_err(|error| CryptoError::InvalidHex(error.to_string()))
}

/// 计算十六进制私钥的摘要前缀。
fn checksum_hex(private_key_hex: &str) -> String {
    let digest = Sha256::digest(private_key_hex.as_bytes());
    hex::encode(&digest[..8])
}

/// 对钱包结构做完整性校验，避免导入损坏数据。
fn validate_wallet_integrity(wallet: &Wallet) -> CryptoResult<()> {
    if wallet.address.trim().is_empty() {
        return Err(CryptoError::InvalidWallet("address 不能为空".to_string()));
    }
    if wallet.public_key.trim().is_empty() {
        return Err(CryptoError::InvalidWallet(
            "public_key 不能为空".to_string(),
        ));
    }
    if wallet.encrypted_private_key.trim().is_empty() {
        return Err(CryptoError::InvalidWallet(
            "encrypted_private_key 不能为空".to_string(),
        ));
    }
    if wallet.kdf_salt.trim().is_empty() {
        return Err(CryptoError::InvalidWallet("kdf_salt 不能为空".to_string()));
    }
    if wallet.private_key_checksum.trim().is_empty() {
        return Err(CryptoError::InvalidWallet(
            "private_key_checksum 不能为空".to_string(),
        ));
    }

    let derived_address = derive_address_from_public_key(&wallet.public_key)?;
    if wallet.address != derived_address {
        return Err(CryptoError::InvalidWallet(
            "address 与 public_key 不匹配".to_string(),
        ));
    }

    let encrypted = decode_hex(&wallet.encrypted_private_key)?;
    if encrypted.len() != 32 {
        return Err(CryptoError::InvalidWallet(format!(
            "encrypted_private_key 长度不合法: {}",
            encrypted.len()
        )));
    }

    let salt = decode_hex(&wallet.kdf_salt)?;
    if salt.len() != 32 {
        return Err(CryptoError::InvalidWallet(format!(
            "kdf_salt 长度不合法: {}",
            salt.len()
        )));
    }

    let checksum = decode_hex(&wallet.private_key_checksum)?;
    if checksum.len() != 8 {
        return Err(CryptoError::InvalidWallet(format!(
            "private_key_checksum 长度不合法: {}",
            checksum.len()
        )));
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    /// 验证钱包创建、解密与验签流程可用。
    #[test]
    fn wallet_create_sign_verify_should_pass() {
        let (wallet, key_pair) = create_wallet("wallet-pass").expect("创建钱包应当成功");
        assert_eq!(wallet.address, key_pair.address);
        assert_eq!(wallet.public_key, key_pair.public_key);

        let decrypted = wallet
            .decrypt_private_key("wallet-pass")
            .expect("解密应当成功");
        assert_eq!(decrypted, key_pair.private_key);

        let signature = wallet
            .sign_message(b"rustchain-wallet-message", "wallet-pass")
            .expect("签名应当成功");
        let verified = wallet
            .verify_message(b"rustchain-wallet-message", &signature)
            .expect("验签应当成功");
        assert!(verified);
    }

    /// 验证错误密码不会得到正确私钥。
    #[test]
    fn wrong_password_should_not_restore_private_key() {
        let (wallet, key_pair) = create_wallet("wallet-pass").expect("创建钱包应当成功");
        let decrypted = wallet.decrypt_private_key("wrong-pass");

        assert!(matches!(decrypted, Err(CryptoError::WalletDecryptFailed)));
        assert!(!key_pair.private_key.is_empty());
    }

    /// 验证地址派生函数和钱包生成结果一致。
    #[test]
    fn derive_address_from_public_key_should_match_wallet_address() {
        let (wallet, _) = create_wallet("wallet-pass").expect("创建钱包应当成功");
        let address = derive_address_from_public_key(&wallet.public_key).expect("派生地址应当成功");

        assert_eq!(address, wallet.address);
    }

    /// 验证可以通过私钥导入钱包，并得到一致地址。
    #[test]
    fn create_wallet_from_private_key_should_keep_same_address() {
        let (wallet, key_pair) = create_wallet("wallet-pass").expect("创建钱包应当成功");
        let (imported_wallet, imported_key_pair) =
            create_wallet_from_private_key(&key_pair.private_key, "new-pass")
                .expect("导入钱包应当成功");

        assert_eq!(wallet.address, imported_wallet.address);
        assert_eq!(wallet.public_key, imported_wallet.public_key);
        assert_eq!(key_pair.address, imported_key_pair.address);
    }

    /// 验证钱包导出与导入 JSON 后数据一致。
    #[test]
    fn wallet_export_import_json_should_roundtrip() {
        let (wallet, _) = create_wallet("wallet-pass").expect("创建钱包应当成功");
        let raw = export_wallet_to_json(&wallet).expect("导出 JSON 应当成功");
        let restored = import_wallet_from_json(&raw).expect("导入 JSON 应当成功");

        assert_eq!(wallet, restored);
    }

    /// 验证钱包备份文件和恢复流程可用。
    #[test]
    fn wallet_backup_and_restore_file_should_work() {
        let (wallet, _) = create_wallet("wallet-pass").expect("创建钱包应当成功");
        let path = temp_wallet_file_path("wallet-backup");
        backup_wallet_to_file(&wallet, &path).expect("备份钱包应当成功");

        let restored = restore_wallet_from_file(&path).expect("恢复钱包应当成功");
        assert_eq!(wallet, restored);

        let _ = fs::remove_file(path);
    }

    /// 验证导入被篡改地址的钱包会失败。
    #[test]
    fn import_wallet_with_tampered_address_should_fail() {
        let (wallet, _) = create_wallet("wallet-pass").expect("创建钱包应当成功");
        let mut raw = export_wallet_to_json(&wallet).expect("导出 JSON 应当成功");
        raw = raw.replace(
            &wallet.address,
            "0x0000000000000000000000000000000000000000",
        );

        let result = import_wallet_from_json(&raw);
        assert!(matches!(result, Err(CryptoError::InvalidWallet(_))));
    }

    /// 生成测试临时钱包文件路径。
    fn temp_wallet_file_path(prefix: &str) -> PathBuf {
        let ts = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .expect("系统时间应可用")
            .as_nanos();
        std::env::temp_dir().join(format!("{prefix}-{ts}.json"))
    }
}
