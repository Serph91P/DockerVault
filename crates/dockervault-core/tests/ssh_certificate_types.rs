use base64::{Engine, engine::general_purpose::STANDARD};
use dockervault_core::ssh_public::canonical_public_key;
use serde_json::Value;
#[test]
fn unsupported_certificate_types_are_not_accepted_as_portable_metadata() {
    let fixture: Value = serde_json::from_str(include_str!(
        "fixtures/rclone-public-material.TEST_ONLY.json"
    ))
    .unwrap();
    for vector in fixture["metadata_type_controls"].as_array().unwrap() {
        assert_eq!(vector["rejected"], true);
        let raw = STANDARD
            .decode(vector["raw_base64"].as_str().unwrap())
            .unwrap();
        assert!(canonical_public_key(&raw).is_err(), "{}", vector["name"]);
    }
}
