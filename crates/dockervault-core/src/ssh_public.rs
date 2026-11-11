//! Public-only SSH metadata normalization. Parsing a certificate does not authorize
//! its signer, principal or validity period; this is not an authentication verifier.
mod certificate;
use base64::{Engine, engine::general_purpose::STANDARD};
use openssl::{
    bn::{BigNum, BigNumContext},
    ec::{EcGroup, EcKey, EcPoint},
    nid::Nid,
};
use ssh_key::{
    PublicKey,
    public::{EcdsaPublicKey, KeyData},
};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct PublicKeyError;
impl std::fmt::Display for PublicKeyError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Invalid or incompatible recovery state")
    }
}
impl std::error::Error for PublicKeyError {}
impl From<ssh_key::Error> for PublicKeyError {
    fn from(_: ssh_key::Error) -> Self {
        Self
    }
}
impl From<openssl::error::ErrorStack> for PublicKeyError {
    fn from(_: openssl::error::ErrorStack) -> Self {
        Self
    }
}

pub fn canonical_public_key(raw: &[u8]) -> Result<String, PublicKeyError> {
    if raw.len() > 16384 {
        return Err(PublicKeyError);
    }
    canonical_inner(raw)
}
/// rclone's identity parser excludes legacy DSA fallback and permits its larger
/// material budget; this still does not verify a certificate's authorization.
pub fn canonical_public_identity(raw: &[u8]) -> Result<String, PublicKeyError> {
    if raw.len() > 4 * 1024 * 1024 {
        return Err(PublicKeyError);
    }
    let public = canonical_inner(raw)?;
    let algorithm = raw
        .split(|b| b.is_ascii_whitespace())
        .next()
        .ok_or(PublicKeyError)?;
    if public.starts_with("ssh-dss ")
        || (algorithm.ends_with(b"-cert-v01@openssh.com")
            && !public.as_bytes().starts_with(algorithm))
    {
        return Err(PublicKeyError);
    }
    Ok(public)
}
fn canonical_inner(raw: &[u8]) -> Result<String, PublicKeyError> {
    if raw.is_empty() || raw[0].is_ascii_whitespace() {
        return Err(PublicKeyError);
    }
    let end = raw
        .iter()
        .position(u8::is_ascii_whitespace)
        .ok_or(PublicKeyError)?;
    if !matches!(raw[end], b' ' | b'\t') {
        return Err(PublicKeyError);
    }
    let algorithm = std::str::from_utf8(&raw[..end]).map_err(|_| PublicKeyError)?;
    let rest = &raw[end..];
    let start = rest
        .iter()
        .position(|b| !matches!(b, b' ' | b'\t'))
        .ok_or(PublicKeyError)?;
    let rest = &rest[start..];
    let end = rest
        .iter()
        .position(u8::is_ascii_whitespace)
        .unwrap_or(rest.len());
    // The provider's binascii decoder ignores non-alphabet bytes. Re-encode before
    // the structured codec, and never pass comments/following lines to that codec.
    let binary = decode_public_base64(&rest[..end])?;
    let binary = match algorithm {
        "ssh-rsa" => normalize_integers(&binary, algorithm, 2)?,
        "ssh-dss" => normalize_integers(&binary, algorithm, 4)?,
        _ => binary,
    };
    let public = format!("{algorithm} {}", STANDARD.encode(&binary));
    if algorithm.ends_with("-cert-v01@openssh.com") {
        return certificate::canonical(algorithm, &binary);
    }
    let public = PublicKey::from_openssh(&public)?;
    validate(public.key_data())?;
    let key = match public.key_data() {
        KeyData::SkEd25519(key) => KeyData::Ed25519(*key.public_key()),
        KeyData::SkEcdsaSha2NistP256(key) => {
            KeyData::Ecdsa(EcdsaPublicKey::NistP256(*key.ec_point()))
        }
        key => key.clone(),
    };
    Ok(PublicKey::new(key, "").to_openssh()?)
}
fn decode_public_base64(input: &[u8]) -> Result<Vec<u8>, PublicKeyError> {
    let mut encoded = Vec::with_capacity(input.len());
    let mut padding = 0;
    for &byte in input {
        if byte == b'=' {
            padding += 1;
        } else if byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/') {
            padding = 0;
            encoded.push(byte);
        }
    }
    let remainder = encoded.len() % 4;
    if remainder == 1 || (remainder != 0 && remainder + padding < 4) {
        return Err(PublicKeyError);
    }
    if remainder != 0 {
        encoded.extend(std::iter::repeat_n(b'=', 4 - remainder));
    }
    let engine = base64::engine::GeneralPurpose::new(
        &base64::alphabet::STANDARD,
        base64::engine::GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true),
    );
    engine.decode(encoded).map_err(|_| PublicKeyError)
}
fn field<'a>(input: &mut &'a [u8]) -> Result<&'a [u8], PublicKeyError> {
    let header = input.get(..4).ok_or(PublicKeyError)?;
    let len = u32::from_be_bytes(header.try_into().map_err(|_| PublicKeyError)?) as usize;
    let data = input.get(4..).ok_or(PublicKeyError)?;
    let result = data.get(..len).ok_or(PublicKeyError)?;
    *input = &data[len..];
    Ok(result)
}
fn normalize_integers(
    binary: &[u8],
    algorithm: &str,
    count: usize,
) -> Result<Vec<u8>, PublicKeyError> {
    // The provider accepts redundant leading zeroes (including a one-byte zero),
    // while ssh-key correctly demands RFC4251 canonical mpints. Normalize before it.

    fn append(output: &mut Vec<u8>, value: &[u8]) {
        output.extend_from_slice(&(value.len() as u32).to_be_bytes());
        output.extend_from_slice(value);
    }
    let mut input = binary;
    let label = field(&mut input)?;
    if label != algorithm.as_bytes() {
        return Err(PublicKeyError);
    }
    let mut output = Vec::with_capacity(binary.len());
    append(&mut output, label);
    for _ in 0..count {
        let bytes = field(&mut input)?;
        if bytes.first().is_some_and(|b| b & 0x80 != 0) {
            return Err(PublicKeyError);
        }
        let integer = ssh_key::Mpint::from_positive_bytes(bytes)?;
        append(&mut output, integer.as_bytes());
    }
    if !input.is_empty() {
        return Err(PublicKeyError);
    }
    Ok(output)
}
fn number(value: &ssh_key::Mpint) -> Result<BigNum, PublicKeyError> {
    let bytes = value.as_bytes();
    if bytes.first().is_some_and(|b| b & 0x80 != 0) {
        return Err(PublicKeyError);
    }
    Ok(BigNum::from_slice(bytes)?)
}
fn validate(key: &KeyData) -> Result<(), PublicKeyError> {
    match key {
        KeyData::Rsa(key) => {
            let e = number(&key.e)?;
            let n = number(&key.n)?;
            let three = BigNum::from_u32(3)?;
            if e.ucmp(&three).is_lt()
                || n.ucmp(&three).is_lt()
                || !e.is_bit_set(0)
                || !e.ucmp(&n).is_lt()
            {
                return Err(PublicKeyError);
            }
        }
        KeyData::Dsa(key) => {
            let p = number(&key.p)?;
            let q = number(&key.q)?;
            let g = number(&key.g)?;
            let _y = number(&key.y)?;
            let one = BigNum::from_u32(1)?;
            if p.num_bits() != 1024
                || q.num_bits() != 160
                || !g.ucmp(&one).is_gt()
                || !g.ucmp(&p).is_lt()
            {
                return Err(PublicKeyError);
            }
        }
        KeyData::Ecdsa(key) => validate_ec(key)?,
        KeyData::Ed25519(_) => {}
        KeyData::SkEd25519(_) => {}
        KeyData::SkEcdsaSha2NistP256(key) => {
            validate_ec(&EcdsaPublicKey::NistP256(*key.ec_point()))?
        }
        _ => return Err(PublicKeyError),
    }
    Ok(())
}
fn validate_ec(key: &EcdsaPublicKey) -> Result<(), PublicKeyError> {
    let curve = match key {
        EcdsaPublicKey::NistP256(_) => Nid::X9_62_PRIME256V1,
        EcdsaPublicKey::NistP384(_) => Nid::SECP384R1,
        EcdsaPublicKey::NistP521(_) => Nid::SECP521R1,
    };
    let bytes = key.as_sec1_bytes();
    if bytes.first() != Some(&4) {
        return Err(PublicKeyError);
    }
    let group = EcGroup::from_curve_name(curve)?;
    let mut context = BigNumContext::new()?;
    let point = EcPoint::from_bytes(&group, bytes, &mut context)?;
    EcKey::from_public_key(&group, &point)?.check_key()?;
    Ok(())
}
