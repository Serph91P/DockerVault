//! Opaque OpenSSH certificate metadata framing, not certificate authorization.
use super::{PublicKeyError, canonical_inner, field};
use base64::{Engine, engine::general_purpose::STANDARD};

pub(super) fn canonical(algorithm: &str, binary: &[u8]) -> Result<String, PublicKeyError> {
    let mut input = binary;
    if field(&mut input)? != algorithm.as_bytes() {
        return Err(PublicKeyError);
    }
    field(&mut input)?; // nonce
    let base = algorithm
        .strip_suffix("-cert-v01@openssh.com")
        .ok_or(PublicKeyError)?;
    let count = match base {
        "ssh-rsa" => 2,
        "ssh-dss" => 4,
        "ssh-ed25519" => 1,
        "ecdsa-sha2-nistp256" | "ecdsa-sha2-nistp384" | "ecdsa-sha2-nistp521" => 2,
        "sk-ssh-ed25519@openssh.com" => 2,
        "sk-ecdsa-sha2-nistp256@openssh.com" => 3,
        _ => return Err(PublicKeyError),
    };

    let fields = input;
    for _ in 0..count {
        field(&mut input)?;
    }
    let fields = &fields[..fields.len() - input.len()];
    let mut subject = Vec::with_capacity(4 + base.len() + fields.len());
    subject.extend_from_slice(&(base.len() as u32).to_be_bytes());
    subject.extend_from_slice(base.as_bytes());
    subject.extend_from_slice(fields);
    let public = canonical_inner(format!("{base} {}", STANDARD.encode(subject)).as_bytes())?;
    take(&mut input, 8)?; // serial is retained, not authorized here
    let certificate_type = u32::from_be_bytes(
        input
            .get(..4)
            .ok_or(PublicKeyError)?
            .try_into()
            .map_err(|_| PublicKeyError)?,
    );
    if !matches!(certificate_type, 1 | 2) {
        return Err(PublicKeyError);
    }
    take(&mut input, 4)?;
    field(&mut input)?; // opaque key ID
    let mut principals = field(&mut input)?;
    while !principals.is_empty() {
        field(&mut principals)?;
    }
    take(&mut input, 16)?; // validity window, retained rather than interpreted
    options(field(&mut input)?)?;
    options(field(&mut input)?)?;
    field(&mut input)?; // reserved
    let mut issuer = field(&mut input)?;
    let issuer_algorithm = field(&mut issuer)?;
    let mut signature = field(&mut input)?;
    if !input.is_empty() {
        return Err(PublicKeyError);
    }
    let signature_algorithm = field(&mut signature)?;
    if if issuer_algorithm == b"ssh-rsa" {
        ![b"ssh-rsa".as_slice(), b"rsa-sha2-256", b"rsa-sha2-512"].contains(&signature_algorithm)
    } else {
        signature_algorithm != issuer_algorithm
    } {
        return Err(PublicKeyError);
    }
    field(&mut signature)?;
    if !signature.is_empty() {
        return Err(PublicKeyError);
    }
    if base == "ssh-dss" || issuer_algorithm == b"ssh-dss" {
        return Ok(public);
    }
    // Original signed bytes include binary metadata which a text-only certificate
    // codec cannot retain. Every input comment/following line is already excluded.
    Ok(format!("{algorithm} {}", STANDARD.encode(binary)))
}
fn take(input: &mut &[u8], length: usize) -> Result<(), PublicKeyError> {
    *input = input.get(length..).ok_or(PublicKeyError)?;
    Ok(())
}
fn options(mut input: &[u8]) -> Result<(), PublicKeyError> {
    let mut previous: Option<&[u8]> = None;
    while !input.is_empty() {
        let name = field(&mut input)?;
        if previous.is_some_and(|previous| previous >= name) {
            return Err(PublicKeyError);
        }
        let mut value = field(&mut input)?;
        if !value.is_empty() {
            field(&mut value)?;
            if !value.is_empty() {
                return Err(PublicKeyError);
            }
        }
        previous = Some(name);
    }
    Ok(())
}
