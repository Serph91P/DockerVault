//! Python's existing non-validating URL-safe Base64 framing only. Authentication
//! and all Fernet cryptography remain in the qualified Fernet implementation.
use base64::{
    Engine, alphabet,
    engine::{GeneralPurpose, GeneralPurposeConfig, general_purpose::URL_SAFE},
};
use zeroize::Zeroizing;

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
