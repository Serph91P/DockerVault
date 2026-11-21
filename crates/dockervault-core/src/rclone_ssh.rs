//! Public-only rclone SSH material, never a private-key reader or trust decision.
use crate::ssh_public::{PublicKeyError, canonical_public_identity};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum SshMaterial {
    PublicIdentity,
    KnownHosts,
}
pub fn canonical_material(data: &[u8], kind: SshMaterial) -> Result<Vec<u8>, PublicKeyError> {
    // Source inline options are bounded at 1Mi Unicode characters; file material
    // has the tighter 1MiB byte bound enforced by the material collector.
    if data.len() > 4 * 1024 * 1024 {
        return Err(PublicKeyError);
    }
    let mut lines = data
        .split(|b| matches!(b, b'\n' | b'\r'))
        .map(|line| line.trim_ascii())
        .filter(|line| !line.is_empty());
    if kind == SshMaterial::PublicIdentity {
        return Ok(canonical_public_identity(lines.next().ok_or(PublicKeyError)?)?.into_bytes());
    }
    let mut output = Vec::new();
    for line in lines {
        if line.starts_with(b"#") {
            continue;
        }
        let mut parts = line
            .split(u8::is_ascii_whitespace)
            .filter(|part| !part.is_empty());
        let first = parts.next().ok_or(PublicKeyError)?;
        let host = if first.starts_with(b"@") {
            if !matches!(first, b"@cert-authority" | b"@revoked") {
                return Err(PublicKeyError);
            }
            output.extend_from_slice(first);
            output.push(b' ');
            parts.next().ok_or(PublicKeyError)?
        } else {
            first
        };
        let algorithm = parts.next().ok_or(PublicKeyError)?;
        let key = parts.next().ok_or(PublicKeyError)?;
        let mut public = Vec::with_capacity(algorithm.len() + 1 + key.len());
        public.extend_from_slice(algorithm);
        public.push(b' ');
        public.extend_from_slice(key);
        let public = canonical_public_identity(&public)?;
        output.extend_from_slice(host);
        output.push(b' ');
        output.extend_from_slice(public.as_bytes());
        output.push(b'\n');
    }
    if output.is_empty() {
        return Err(PublicKeyError);
    }
    Ok(output)
}
