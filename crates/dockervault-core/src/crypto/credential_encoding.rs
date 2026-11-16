//! Python's existing non-validating URL-safe Base64 framing only. Authentication
//! and all Fernet cryptography remain in the qualified Fernet implementation.
use base64::{
    Engine, alphabet,
    engine::{GeneralPurpose, GeneralPurposeConfig, general_purpose::URL_SAFE},
};
use zeroize::Zeroizing;

pub(super) fn read_key(
    mut reader: impl std::io::Read,
    cancelled: &dyn Fn() -> bool,
) -> Result<Zeroizing<String>, super::CryptoError> {
    use super::CryptoError::{Cancelled, InvalidKey};
    let mut buffer = Zeroizing::new([0u8; 8192]);
    let mut tail = Zeroizing::new(Vec::new());
    let mut encoded = Zeroizing::new(String::with_capacity(44));
    let mut padded = false;
    loop {
        if cancelled() {
            return Err(Cancelled);
        }
        let count = match reader.read(buffer.as_mut_slice()) {
            Ok(n) => n,
            Err(error) if error.kind() == std::io::ErrorKind::Interrupted => continue,
            Err(error) => return Err(error.into()),
        };
        if count == 0 {
            break;
        }
        let mut text = Zeroizing::new(Vec::with_capacity(tail.len() + count));
        text.extend_from_slice(&tail);
        text.extend_from_slice(&buffer[..count]);
        match std::str::from_utf8(&text) {
            Ok(_) => tail.clear(),
            Err(error) if error.error_len().is_none() => {
                tail.clear();
                tail.extend_from_slice(&text[error.valid_up_to()..]);
            }
            Err(_) => return Err(InvalidKey),
        }
        for &byte in &buffer[..count] {
            if byte == b'=' {
                padded = true;
            } else if byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/' | b'-' | b'_') {
                // Exactly 43 alphabet bytes encode a 32-byte Fernet key. In the
                // accepted decoder, interior padding never erases alphabet data.
                if encoded.len() == 43 {
                    return Err(InvalidKey);
                }
                encoded.push(char::from(byte));
                padded = false;
            }
        }
    }
    if !tail.is_empty() || encoded.len() != 43 || !padded {
        return Err(InvalidKey);
    }
    encoded.push('=');
    Ok(encoded)
}

pub(super) fn canonical(input: &[u8]) -> Result<Zeroizing<String>, ()> {
    let mut encoded = Zeroizing::new(Vec::new());
    let mut padding = 0;
    for &byte in input {
        let byte = match byte {
            b'-' => b'+',
            b'_' => b'/',
            other => other,
        };
        if byte == b'=' {
            padding += 1;
        } else if byte.is_ascii_alphanumeric() || matches!(byte, b'+' | b'/') {
            encoded.push(byte);
            padding = 0;
        }
    }
    let remainder = encoded.len() % 4;
    if remainder == 1 || (remainder != 0 && remainder + padding < 4) {
        return Err(());
    }
    if remainder != 0 {
        encoded.extend(std::iter::repeat_n(b'=', 4 - remainder));
    }
    let engine = GeneralPurpose::new(
        &alphabet::STANDARD,
        GeneralPurposeConfig::new().with_decode_allow_trailing_bits(true),
    );
    let decoded = Zeroizing::new(engine.decode(encoded.as_slice()).map_err(|_| ())?);
    Ok(Zeroizing::new(URL_SAFE.encode(decoded.as_slice())))
}
