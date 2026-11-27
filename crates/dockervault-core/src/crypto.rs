//! Existing encryption formats only; cryptographic primitives stay in qualified libraries.
mod credential_encoding;
mod envelope;
pub use envelope::{decrypt_dek, decrypt_dek_file, encrypt_dek};
const KEY_MAGIC: &[u8; 8] = b"DVKEY\0\x01\n";
#[cfg(unix)]
mod archive;
#[cfg(unix)]
pub use archive::{decrypt_file, decrypt_private, encrypt_file, encrypt_stream, key_sidecar_path};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CryptoError {
    InvalidKey,
    Authentication,
    Encoding,
    Format,
    Cancelled,
    Io,
    Crypto,
    PublicationUncertain,
}
impl std::fmt::Display for CryptoError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::InvalidKey => "Invalid encryption key",
            Self::Authentication => "Encrypted data authentication failed",
            Self::Encoding => "Decrypted credential is not UTF-8",
            Self::Format => "Invalid or unsupported archive/key format",
            Self::Cancelled => "Cryptographic work cancelled",
            Self::Io => "Cryptographic file I/O failed",
            Self::Crypto => "Cryptographic operation failed",
            Self::PublicationUncertain => {
                "Valid output published but directory durability uncertain"
            }
        })
    }
}
impl std::error::Error for CryptoError {}
impl From<std::io::Error> for CryptoError {
    fn from(_: std::io::Error) -> Self {
        Self::Io
    }
}
impl From<openssl::error::ErrorStack> for CryptoError {
    fn from(_: openssl::error::ErrorStack) -> Self {
        Self::Crypto
    }
}

pub struct CredentialCipher(fernet::Fernet);
impl std::fmt::Debug for CredentialCipher {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("CredentialCipher([REDACTED])")
    }
}
impl CredentialCipher {
    pub fn generate_key() -> zeroize::Zeroizing<String> {
        zeroize::Zeroizing::new(fernet::Fernet::generate_key())
    }
    pub fn new(key: &str) -> Result<Self, CryptoError> {
        let canonical =
            credential_encoding::canonical(key.as_bytes()).map_err(|_| CryptoError::InvalidKey)?;
        fernet::Fernet::new(&canonical)
            .map(Self)
            .ok_or(CryptoError::InvalidKey)
    }
    /// Read existing UTF-8 key text with fixed scratch space; never generate or
    /// publish a replacement key. The caller owns I/O and custody/permissions.
    pub fn read_key(
        reader: impl std::io::Read,
        cancelled: impl Fn() -> bool,
    ) -> Result<Self, CryptoError> {
        let key = credential_encoding::read_key(reader, &cancelled)?;
        if cancelled() {
            return Err(CryptoError::Cancelled);
        }
        Self::new(&key)
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
        let canonical = credential_encoding::canonical(token.as_bytes())
            .map_err(|_| CryptoError::Authentication)?;
        let bytes = self
            .0
            .decrypt(&canonical)
            .map_err(|_| CryptoError::Authentication)?;
        String::from_utf8(bytes).map_err(|_| CryptoError::Encoding)
    }
}
