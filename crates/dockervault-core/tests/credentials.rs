use dockervault_core::crypto::CredentialCipher;
use serde_json::Value;
const PUBLIC: &str = include_str!("fixtures/credential-vectors.TEST_ONLY.json");
#[test]
fn frozen_python_fernet_and_legacy_empty_plaintext_semantics_are_preserved() {
    let fixture: Value = serde_json::from_str(PUBLIC).unwrap();
    let cipher = CredentialCipher::new(fixture["fernet_key"].as_str().unwrap()).unwrap();
    let token = fixture["fernet_ciphertext"].as_str().unwrap();
    assert_eq!(
        cipher.decrypt_value(token).unwrap(),
        fixture["fernet_plaintext"].as_str().unwrap()
    );
    assert_eq!(cipher.encrypt_value(""), "");
    assert_eq!(cipher.decrypt_value("").unwrap(), "");
    for legacy in [
        "ordinary legacy value",
        " fernet:not-a-prefixed-value",
        "日本語",
    ] {
        assert_eq!(cipher.decrypt_value(legacy).unwrap(), legacy);
    }
    assert_eq!(format!("{cipher:?}"), "CredentialCipher([REDACTED])");
}
#[test]
fn native_fernet_tokens_roundtrip_utf8_and_export_only_public_synthetic_oracle_data() {
    let fixture: Value = serde_json::from_str(PUBLIC).unwrap();
    let cipher = CredentialCipher::new(fixture["fernet_key"].as_str().unwrap()).unwrap();
    let plain = "PUBLIC TEST ONLY: \0\n雪😀";
    let token = cipher.encrypt_value(plain);
    assert!(token.starts_with("fernet:"));
    assert_eq!(cipher.decrypt_value(&token).unwrap(), plain);
    let file = std::env::temp_dir().join("native-fernet.TEST_ONLY.json");
    std::fs::write(file,serde_json::to_vec(&serde_json::json!({"token":token,"plaintext":plain,"warning":"PUBLIC SYNTHETIC TEST MATERIAL ONLY"})).unwrap()).unwrap();
}
#[test]
fn prefixed_malformed_wrong_key_and_tampered_tokens_never_fall_back_to_plaintext() {
    assert!(CredentialCipher::new("invalid").is_err());
    let fixture: Value = serde_json::from_str(PUBLIC).unwrap();
    let cipher = CredentialCipher::new(fixture["fernet_key"].as_str().unwrap()).unwrap();
    for token in ["fernet:", "fernet:invalid", "fernet:雪"] {
        assert!(cipher.decrypt_value(token).is_err());
    }
    let token = cipher.encrypt_value("PUBLIC");
    let mut changed = token.clone().into_bytes();
    let index = changed.len() / 2;
    changed[index] ^= 1;
    assert!(
        cipher
            .decrypt_value(std::str::from_utf8(&changed).unwrap())
            .is_err()
    );
    assert!(cipher.decrypt_value(&token[..token.len() - 8]).is_err());
    let wrong = CredentialCipher::new(&fernet::Fernet::generate_key()).unwrap();
    assert!(wrong.decrypt_value(&token).is_err());
}
#[test]
fn generated_instance_keys_have_the_existing_fernet_wire_shape_and_fresh_entropy() {
    use base64::{Engine, engine::general_purpose::URL_SAFE};
    let first = CredentialCipher::generate_key();
    let second = CredentialCipher::generate_key();
    assert_eq!(first.len(), 44);
    assert_eq!(URL_SAFE.decode(first.as_bytes()).unwrap().len(), 32);
    assert!(first.as_bytes() != second.as_bytes());
    assert!(CredentialCipher::new(&first).is_ok());
    assert!(CredentialCipher::new(&second).is_ok());
}
