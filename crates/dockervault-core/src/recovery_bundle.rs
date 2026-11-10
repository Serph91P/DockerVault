//! The accepted fc1b339 portable recovery envelope, without application state or filesystem effects.
use argon2::{Algorithm, Argon2, Params, Version};
use openssl::{
    base64::{decode_block, encode_block},
    rand::rand_bytes,
    symm::{Cipher, decrypt_aead, encrypt_aead},
};
use serde::{
    Deserialize, Serialize,
    de::{self, MapAccess, SeqAccess, Visitor},
};
use serde_json::{Value, json};
use std::{
    collections::BTreeSet,
    fmt,
    sync::atomic::{AtomicBool, Ordering},
};
use zeroize::Zeroizing;

const MAX_BUNDLE: usize = 32 * 1024 * 1024;
const MAX_PAYLOAD: usize = 24 * 1024 * 1024 - 1024;
static KDF_BUSY: AtomicBool = AtomicBool::new(false);
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum BundleError {
    Invalid,
    Busy,
    Internal,
}
impl fmt::Display for BundleError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Invalid => "Invalid recovery bundle or password",
            Self::Busy => "Recovery cryptography is busy; try again",
            Self::Internal => "Recovery cryptography failed",
        })
    }
}
impl std::error::Error for BundleError {}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Envelope {
    format: String,
    envelope_version: u32,
    kdf: Kdf,
    aead: Aead,
    ciphertext_b64: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Kdf {
    name: String,
    memory_kib: u32,
    iterations: u32,
    parallelism: u32,
    salt_b64: String,
}
#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct Aead {
    name: String,
    nonce_b64: String,
    tag_b64: String,
}
struct KdfSlot;
impl Drop for KdfSlot {
    fn drop(&mut self) {
        KDF_BUSY.store(false, Ordering::Release);
    }
}
fn password(value: &str) -> Result<(), BundleError> {
    if value.is_empty() || value.len() > 1024 {
        Err(BundleError::Invalid)
    } else {
        Ok(())
    }
}
fn key(password: &str, salt: &[u8]) -> Result<Zeroizing<[u8; 32]>, BundleError> {
    KDF_BUSY
        .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
        .map_err(|_| BundleError::Busy)?;
    let _slot = KdfSlot;
    let params = Params::new(65536, 3, 1, Some(32)).map_err(|_| BundleError::Internal)?;
    let mut key = Zeroizing::new([0; 32]);
    Argon2::new(Algorithm::Argon2id, Version::V0x13, params)
        .hash_password_into(password.as_bytes(), salt, &mut *key)
        .map_err(|_| BundleError::Internal)?;
    Ok(key)
}
fn b64(value: &str, length: Option<usize>) -> Result<Vec<u8>, BundleError> {
    let raw = decode_block(value).map_err(|_| BundleError::Invalid)?;
    if raw.is_empty()
        || encode_block(&raw) != value
        || length.is_some_and(|length| raw.len() != length)
    {
        return Err(BundleError::Invalid);
    }
    Ok(raw)
}
fn canonical(mut value: Value) -> Result<Vec<u8>, BundleError> {
    value.sort_all_objects();
    let bytes = crate::encoding::python_json_compact(&value).into_bytes();
    serde_json::from_slice::<de::IgnoredAny>(&bytes).map_err(|_| BundleError::Invalid)?;
    Ok(bytes)
}
fn aad(envelope: &Envelope) -> Result<Vec<u8>, BundleError> {
    canonical(
        json!({"format":envelope.format,"envelope_version":envelope.envelope_version,"kdf":envelope.kdf,"aead":{"name":envelope.aead.name,"nonce_b64":envelope.aead.nonce_b64}}),
    )
}

/// CPU-bound native work: the service must keep its physical blocking/crypto owner through return.
pub fn encode_bundle(payload: Value, secret: &str) -> Result<Vec<u8>, BundleError> {
    password(secret)?;
    if !payload.is_object() {
        return Err(BundleError::Invalid);
    }
    let plaintext = Zeroizing::new(canonical(payload)?);
    if plaintext.len() > MAX_PAYLOAD {
        return Err(BundleError::Invalid);
    }
    let mut salt = [0; 16];
    let mut nonce = [0; 12];
    rand_bytes(&mut salt).map_err(|_| BundleError::Internal)?;
    rand_bytes(&mut nonce).map_err(|_| BundleError::Internal)?;
    let mut envelope = Envelope {
        format: "dockervault-recovery".into(),
        envelope_version: 1,
        kdf: Kdf {
            name: "argon2id".into(),
            memory_kib: 65536,
            iterations: 3,
            parallelism: 1,
            salt_b64: encode_block(&salt),
        },
        aead: Aead {
            name: "aes-256-gcm".into(),
            nonce_b64: encode_block(&nonce),
            tag_b64: String::new(),
        },
        ciphertext_b64: String::new(),
    };
    let key = key(secret, &salt)?;
    let mut tag = [0; 16];
    let ciphertext = encrypt_aead(
        Cipher::aes_256_gcm(),
        &*key,
        Some(&nonce),
        &aad(&envelope)?,
        &plaintext,
        &mut tag,
    )
    .map_err(|_| BundleError::Internal)?;
    envelope.aead.tag_b64 = encode_block(&tag);
    envelope.ciphertext_b64 = encode_block(&ciphertext);
    let result = canonical(serde_json::to_value(envelope).map_err(|_| BundleError::Internal)?)?;
    if result.len() > MAX_BUNDLE {
        return Err(BundleError::Invalid);
    }
    Ok(result)
}

pub fn decode_bundle(data: &[u8], secret: &str) -> Result<Value, BundleError> {
    password(secret)?;
    if data.is_empty() || data.len() > MAX_BUNDLE {
        return Err(BundleError::Invalid);
    }
    let envelope: Envelope = serde_json::from_slice(data).map_err(|_| BundleError::Invalid)?;
    if envelope.format != "dockervault-recovery"
        || envelope.envelope_version != 1
        || envelope.kdf.name != "argon2id"
        || envelope.aead.name != "aes-256-gcm"
        || envelope.kdf.memory_kib != 65536
        || envelope.kdf.iterations != 3
        || envelope.kdf.parallelism != 1
    {
        return Err(BundleError::Invalid);
    }
    let salt = b64(&envelope.kdf.salt_b64, Some(16))?;
    let nonce = b64(&envelope.aead.nonce_b64, Some(12))?;
    let tag = b64(&envelope.aead.tag_b64, Some(16))?;
    let ciphertext = b64(&envelope.ciphertext_b64, None)?;
    if ciphertext.len() > MAX_PAYLOAD {
        return Err(BundleError::Invalid);
    }
    let key = key(secret, &salt)?;
    let plaintext = Zeroizing::new(
        decrypt_aead(
            Cipher::aes_256_gcm(),
            &*key,
            Some(&nonce),
            &aad(&envelope)?,
            &ciphertext,
            &tag,
        )
        .map_err(|_| BundleError::Invalid)?,
    );
    let mut parser = serde_json::Deserializer::from_slice(&plaintext);
    Shape::deserialize(&mut parser).map_err(|_| BundleError::Invalid)?;
    parser.end().map_err(|_| BundleError::Invalid)?;
    let value: Value = serde_json::from_slice(&plaintext).map_err(|_| BundleError::Invalid)?;
    if !value.is_object() {
        return Err(BundleError::Invalid);
    }
    Ok(value)
}

// Validate duplicates recursively before Value's map parser can discard earlier fields.
struct Shape;
impl<'de> Deserialize<'de> for Shape {
    fn deserialize<D: de::Deserializer<'de>>(parser: D) -> Result<Self, D::Error> {
        parser.deserialize_any(ShapeVisitor).map(|()| Self)
    }
}
struct ShapeVisitor;
impl<'de> Visitor<'de> for ShapeVisitor {
    type Value = ();
    fn expecting(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter.write_str("finite JSON without duplicate properties")
    }
    fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<(), A::Error> {
        let mut keys = BTreeSet::new();
        while let Some(key) = map.next_key::<String>()? {
            if !keys.insert(key) {
                return Err(de::Error::custom("duplicate JSON property"));
            }
            map.next_value::<Shape>()?;
        }
        Ok(())
    }
    fn visit_seq<A: SeqAccess<'de>>(self, mut values: A) -> Result<(), A::Error> {
        while values.next_element::<Shape>()?.is_some() {}
        Ok(())
    }
    fn visit_bool<E: de::Error>(self, _: bool) -> Result<(), E> {
        Ok(())
    }
    fn visit_i64<E: de::Error>(self, _: i64) -> Result<(), E> {
        Ok(())
    }
    fn visit_u64<E: de::Error>(self, _: u64) -> Result<(), E> {
        Ok(())
    }
    fn visit_f64<E: de::Error>(self, value: f64) -> Result<(), E> {
        if value.is_finite() {
            Ok(())
        } else {
            Err(de::Error::custom("nonfinite JSON"))
        }
    }
    fn visit_str<E: de::Error>(self, _: &str) -> Result<(), E> {
        Ok(())
    }
    fn visit_unit<E: de::Error>(self) -> Result<(), E> {
        Ok(())
    }
}
