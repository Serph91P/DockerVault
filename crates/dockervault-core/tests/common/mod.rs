//! PUBLIC SYNTHETIC fixture bytes; never a production key loader or key rotation.
pub fn public_identity(compressed: bool) -> String {
    let data: serde_json::Value = serde_json::from_str(include_str!(
        "../fixtures/age-identity-recipes.TEST_ONLY.json"
    ))
    .unwrap();
    let bytes: Vec<u8> = serde_json::from_value(
        data["recipes"][usize::from(compressed)]["public_test_vector_bytes"].clone(),
    )
    .unwrap();
    assert_eq!(bytes.len(), 32);
    let encoded =
        bech32::encode::<bech32::Bech32>(bech32::Hrp::parse("age-secret-key-").unwrap(), &bytes)
            .unwrap();
    format!("{}\n", encoded.to_ascii_uppercase())
}
