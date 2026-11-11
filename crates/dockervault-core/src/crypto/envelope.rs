use super::{CryptoError, KEY_MAGIC};
use std::io::Read;
use zeroize::Zeroizing;
pub fn encrypt_dek(dek: &[u8], public_key: &str) -> Result<Vec<u8>, CryptoError> {
    if dek.len() != 32 {
        return Err(CryptoError::InvalidKey);
    }
    let recipient: age::x25519::Recipient =
        public_key.parse().map_err(|_| CryptoError::InvalidKey)?;
    let mut material = Zeroizing::new(Vec::with_capacity(40));
    material.extend_from_slice(KEY_MAGIC);
    material.extend_from_slice(dek);
    age::encrypt_and_armor(&recipient, &material)
        .map(String::into_bytes)
        .map_err(|_| CryptoError::Crypto)
}
pub fn decrypt_dek(
    ciphertext: impl Read,
    private_key: &str,
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let identity: age::x25519::Identity = private_key
        .trim()
        .parse()
        .map_err(|_| CryptoError::InvalidKey)?;
    decrypt(ciphertext, std::iter::once(&identity as &dyn age::Identity))
}
pub fn decrypt_dek_file(
    ciphertext: impl Read,
    identity_file: impl std::io::BufRead,
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let identities = age::IdentityFile::from_buffer(identity_file)
        .map_err(|_| CryptoError::InvalidKey)?
        .into_identities()
        .map_err(|_| CryptoError::InvalidKey)?;
    decrypt(
        ciphertext,
        identities
            .iter()
            .map(|identity| identity.as_ref() as &dyn age::Identity),
    )
}
fn decrypt<'a>(
    ciphertext: impl Read,
    identities: impl Iterator<Item = &'a dyn age::Identity>,
) -> Result<Zeroizing<Vec<u8>>, CryptoError> {
    let decryptor = age::Decryptor::new(age::armor::ArmoredReader::new(ciphertext))
        .map_err(|_| CryptoError::Authentication)?;
    let reader = decryptor
        .decrypt(identities)
        .map_err(|_| CryptoError::Authentication)?;
    let mut material = Zeroizing::new(Vec::with_capacity(41));
    reader
        .take(41)
        .read_to_end(&mut material)
        .map_err(|_| CryptoError::Authentication)?;
    let raw = material.strip_prefix(KEY_MAGIC).unwrap_or(&material);
    if raw.len() != 32 {
        return Err(CryptoError::Format);
    }
    Ok(material)
}
