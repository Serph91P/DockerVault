use super::{CryptoError, KEY_MAGIC};
use openssl::{
    hash::MessageDigest,
    pkcs5::pbkdf2_hmac,
    pkey::{PKey, Private},
    sign::Signer,
    symm::{Cipher, Crypter, Mode},
};
use std::{
    fmt::Write as _,
    fs::{self, DirBuilder, File, OpenOptions},
    io::{Read, Write},
    os::unix::fs::{DirBuilderExt, OpenOptionsExt},
    path::{Path, PathBuf},
};
use zeroize::Zeroizing;
const ARCHIVE_MAGIC: &[u8; 8] = b"DVENC\0\x01\n";

const SALTED: &[u8; 8] = b"Salted__";
const CHUNK: usize = 1024 * 1024;
fn checkpoint(cancel: &dyn Fn() -> bool) -> Result<(), CryptoError> {
    if cancel() {
        Err(CryptoError::Cancelled)
    } else {
        Ok(())
    }
}
fn material(key: &[u8]) -> Result<(&[u8], bool), CryptoError> {
    let authenticated = key.starts_with(KEY_MAGIC);
    let raw = if authenticated { &key[8..] } else { key };
    if raw.len() != 32 {
        return Err(CryptoError::InvalidKey);
    }
    Ok((raw, authenticated))
}
fn mac_key(key: &[u8]) -> Result<PKey<Private>, CryptoError> {
    let key = PKey::hmac(key)?;
    let mut signer = Signer::new(MessageDigest::sha256(), &key)?;
    signer.update(b"DockerVault archive authentication v1")?;
    let derived = Zeroizing::new(signer.sign_to_vec()?);
    Ok(PKey::hmac(&derived)?)
}
fn parameters(key: &[u8], salt: &[u8]) -> Result<Zeroizing<[u8; 48]>, CryptoError> {
    let mut password = Zeroizing::new(String::with_capacity(64));
    for byte in key {
        write!(&mut *password, "{byte:02x}").expect("string write");
    }
    let mut output = Zeroizing::new([0u8; 48]);
    pbkdf2_hmac(
        password.as_bytes(),
        salt,
        100_000,
        MessageDigest::sha256(),
        &mut *output,
    )?;
    Ok(output)
}
struct Stage {
    directory: PathBuf,
    path: PathBuf,
    parent: PathBuf,
    file: File,
}
impl Stage {
    fn new(destination: &Path) -> Result<Self, CryptoError> {
        let parent = destination
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .unwrap_or(Path::new("."))
            .to_owned();
        let mut nonce = [0; 16];
        openssl::rand::rand_bytes(&mut nonce)?;
        let directory = parent.join(format!(
            ".dockervault-crypto-{:032x}",
            u128::from_be_bytes(nonce)
        ));
        DirBuilder::new().mode(0o700).create(&directory)?;
        let path = directory.join("output");
        let file = match OpenOptions::new()
            .write(true)
            .create_new(true)
            .mode(0o600)
            .open(&path)
        {
            Ok(file) => file,
            Err(error) => {
                let _ = fs::remove_dir(&directory);
                return Err(error.into());
            }
        };
        Ok(Self {
            directory,
            path,
            parent,
            file,
        })
    }
    fn publish(&self, destination: &Path, cancel: &dyn Fn() -> bool) -> Result<(), CryptoError> {
        self.file.sync_all()?;
        checkpoint(cancel)?;
        let parent = File::open(&self.parent)?;
        parent.sync_all()?;
        checkpoint(cancel)?;
        fs::rename(&self.path, destination)?;
        parent
            .sync_all()
            .map_err(|_| CryptoError::PublicationUncertain)?;
        Ok(())
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.directory);
    }
}

pub fn key_sidecar_path(archive: &str) -> String {
    format!("{}.key", archive.strip_suffix(".enc").unwrap_or(archive))
}
pub fn encrypt_file(
    source: &Path,
    destination: &Path,
    key_material: &[u8],
    cancel: &dyn Fn() -> bool,
) -> Result<(), CryptoError> {
    checkpoint(cancel)?;
    let (key, _) = material(key_material)?;
    let mut input = File::open(source)?;
    let mut salt = [0; 8];
    openssl::rand::rand_bytes(&mut salt)?;
    let parameters = parameters(key, &salt)?;
    let mut cipher = Crypter::new(
        Cipher::aes_256_cbc(),
        Mode::Encrypt,
        &parameters[..32],
        Some(&parameters[32..]),
    )?;
    let signing = mac_key(key)?;
    let mut mac = Signer::new(MessageDigest::sha256(), &signing)?;
    let mut output = Stage::new(destination)?;
    for header in [ARCHIVE_MAGIC.as_slice(), SALTED.as_slice(), salt.as_slice()] {
        mac.update(header)?;
        output.file.write_all(header)?;
    }
    let mut buffer = Zeroizing::new(vec![0; CHUNK]);
    let mut encrypted = vec![0; CHUNK + 16];
    loop {
        checkpoint(cancel)?;
        let count = input.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        let written = cipher.update(&buffer[..count], &mut encrypted)?;
        mac.update(&encrypted[..written])?;
        output.file.write_all(&encrypted[..written])?;
    }
    let written = cipher.finalize(&mut encrypted)?;
    mac.update(&encrypted[..written])?;
    output.file.write_all(&encrypted[..written])?;
    output.file.write_all(&mac.sign_to_vec()?)?;
    checkpoint(cancel)?;
    output.publish(destination, cancel)
}
pub fn decrypt_file(
    source: &Path,
    destination: &Path,
    key_material: &[u8],
    cancel: &dyn Fn() -> bool,
) -> Result<(), CryptoError> {
    checkpoint(cancel)?;
    let (key, require_authentication) = material(key_material)?;
    let mut input = File::open(source)?;
    let size = input.metadata()?.len();
    let mut header = [0; 8];
    input.read_exact(&mut header)?;
    let authenticated = header == *ARCHIVE_MAGIC;
    if !authenticated && (header != *SALTED || require_authentication) {
        return Err(CryptoError::Format);
    }
    let inner_size = if authenticated {
        size.checked_sub(40).ok_or(CryptoError::Format)?
    } else {
        size
    };
    if inner_size < 32 || (inner_size - 16) % 16 != 0 {
        return Err(CryptoError::Format);
    }
    let mut prefix = [0; 8];
    if authenticated {
        input.read_exact(&mut prefix)?;
        if prefix != *SALTED {
            return Err(CryptoError::Format);
        }
    }
    let mut salt = [0; 8];
    input.read_exact(&mut salt)?;
    let parameters = parameters(key, &salt)?;
    let signing = mac_key(key)?;
    let mut mac = Signer::new(MessageDigest::sha256(), &signing)?;
    if authenticated {
        for part in [ARCHIVE_MAGIC.as_slice(), SALTED.as_slice(), salt.as_slice()] {
            mac.update(part)?;
        }
    }
    let mut cipher = Crypter::new(
        Cipher::aes_256_cbc(),
        Mode::Decrypt,
        &parameters[..32],
        Some(&parameters[32..]),
    )?;
    let mut output = Stage::new(destination)?;
    let mut remaining = inner_size - 16;
    let mut buffer = vec![0; CHUNK];
    let mut plaintext = Zeroizing::new(vec![0; CHUNK + 16]);
    // One read feeds both MAC and EVP; only private0600 staging exists until full authentication.
    while remaining > 0 {
        checkpoint(cancel)?;
        let count =
            usize::try_from(remaining.min(CHUNK as u64)).map_err(|_| CryptoError::Format)?;
        input.read_exact(&mut buffer[..count])?;
        if authenticated {
            mac.update(&buffer[..count])?;
        }
        let written = cipher.update(&buffer[..count], &mut plaintext)?;
        output.file.write_all(&plaintext[..written])?;
        remaining -= count as u64;
    }
    if authenticated {
        let mut tag = [0; 32];
        input.read_exact(&mut tag)?;
        if !openssl::memcmp::eq(&tag, &mac.sign_to_vec()?) {
            return Err(CryptoError::Authentication);
        }
    }
    let mut extra = [0];
    if input.read(&mut extra)? != 0 {
        return Err(CryptoError::Format);
    }
    let written = cipher.finalize(&mut plaintext)?;
    output.file.write_all(&plaintext[..written])?;
    checkpoint(cancel)?;
    output.publish(destination, cancel)
}
