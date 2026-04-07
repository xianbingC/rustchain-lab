use crate::{
    error::{CryptoError, CryptoResult},
    wallet::WalletKeyPair,
};
use ed25519_dalek::{Signature, Signer, SigningKey, Verifier, VerifyingKey};

/// ed25519 私钥字节长度。
const SECRET_KEY_LEN: usize = 32;
/// ed25519 公钥字节长度。
const PUBLIC_KEY_LEN: usize = 32;
/// ed25519 签名字节长度。
const SIGNATURE_LEN: usize = 64;

/// 使用十六进制私钥签名消息，并返回十六进制签名。
pub fn sign_message(message: &[u8], private_key_hex: &str) -> CryptoResult<String> {
    let private_key = decode_hex(private_key_hex)?;
    if private_key.len() != SECRET_KEY_LEN {
        return Err(CryptoError::InvalidKeyLength {
            expected: SECRET_KEY_LEN,
            actual: private_key.len(),
        });
    }

    let mut secret_bytes = [0u8; SECRET_KEY_LEN];
    secret_bytes.copy_from_slice(&private_key);

    let signing_key = SigningKey::from_bytes(&secret_bytes);
    let signature = signing_key.sign(message);
    Ok(hex::encode(signature.to_bytes()))
}

/// 使用十六进制公钥和签名验证消息。
pub fn verify_message(
    message: &[u8],
    signature_hex: &str,
    public_key_hex: &str,
) -> CryptoResult<bool> {
    let public_key = decode_hex(public_key_hex)?;
    if public_key.len() != PUBLIC_KEY_LEN {
        return Err(CryptoError::InvalidKeyLength {
            expected: PUBLIC_KEY_LEN,
            actual: public_key.len(),
        });
    }

    let signature_bytes = decode_hex(signature_hex)?;
    if signature_bytes.len() != SIGNATURE_LEN {
        return Err(CryptoError::InvalidSignatureLength {
            expected: SIGNATURE_LEN,
            actual: signature_bytes.len(),
        });
    }

    let mut public_key_array = [0u8; PUBLIC_KEY_LEN];
    public_key_array.copy_from_slice(&public_key);
    let verifying_key = VerifyingKey::from_bytes(&public_key_array)
        .map_err(|_| CryptoError::InvalidSignature)?;

    let mut signature_array = [0u8; SIGNATURE_LEN];
    signature_array.copy_from_slice(&signature_bytes);
    let signature = Signature::from_bytes(&signature_array);

    match verifying_key.verify(message, &signature) {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

/// 使用钱包密钥对完成签名和验签闭环，便于业务层直接调用。
pub fn sign_and_verify_with_key_pair(
    message: &[u8],
    key_pair: &WalletKeyPair,
) -> CryptoResult<bool> {
    let signature = sign_message(message, &key_pair.private_key)?;
    verify_message(message, &signature, &key_pair.public_key)
}

/// 解码十六进制字符串。
fn decode_hex(value: &str) -> CryptoResult<Vec<u8>> {
    hex::decode(value).map_err(|error| CryptoError::InvalidHex(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::wallet::create_wallet;

    /// 验证签名与验签主流程可用。
    #[test]
    fn sign_and_verify_should_pass() {
        let (wallet, key_pair) = create_wallet("pass-123").expect("创建钱包应当成功");
        let message = b"rustchain-signature-check";
        let signature = sign_message(message, &key_pair.private_key).expect("签名应当成功");

        let verified = verify_message(message, &signature, &wallet.public_key)
            .expect("验签应当成功");
        assert!(verified);
    }

    /// 验证消息被篡改后验签失败。
    #[test]
    fn tampered_message_should_fail_verify() {
        let (wallet, key_pair) = create_wallet("pass-123").expect("创建钱包应当成功");
        let signature = sign_message(b"origin", &key_pair.private_key).expect("签名应当成功");

        let verified = verify_message(b"tampered", &signature, &wallet.public_key)
            .expect("验签过程应当可执行");
        assert!(!verified);
    }
}
