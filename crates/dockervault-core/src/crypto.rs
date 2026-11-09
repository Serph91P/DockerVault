//! Existing encryption formats only; cryptographic primitives stay in qualified libraries.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoError {
    InvalidKey,
    Authentication,
    Encoding,
}
impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidKey => "Invalid encryption key",
            Self::Authentication => "Encrypted data authentication failed",
            Self::Encoding => "Decrypted credential is not UTF-8",
        })
    }
}
impl std::error::Error for CryptoError {}

pub struct CredentialCipher(fernet::Fernet);
impl std::fmt::Debug for CredentialCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CredentialCipher([REDACTED])")
    }
}
impl CredentialCipher {
    pub fn new(key: &str) -> Result<Self, CryptoError> {
        fernet::Fernet::new(key)
            .map(Self)
            .ok_or(CryptoError::InvalidKey)
    }
    pub fn encrypt_value(&self, plaintext: &str) -> String {
        if plaintext.is_empty() {
            String::new()
        } else {
            format!("fernet:{}", self.0.encrypt(plaintext.as_bytes()))
        }
    }
    pub fn decrypt_value(&self, ciphertext: &str) -> Result<String, CryptoError> {
        let Some(token) = ciphertext.strip_prefix("fernet:") else {
            return Ok(ciphertext.to_owned());
        };
        let bytes = self
            .0
            .decrypt(token)
            .map_err(|_| CryptoError::Authentication)?;
        String::from_utf8(bytes).map_err(|_| CryptoError::Encoding)
    }
}
