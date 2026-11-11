use base64::{Engine, engine::general_purpose::STANDARD};
use dockervault_core::{
    cron::TimeZone,
    lossless_json::PythonJson,
    recovery_bundle::{decode_json_bundle, encode_json_bundle},
    recovery_state::parse_state_json,
    timestamp::Timestamp,
};
use serde_json::Value;
use std::collections::BTreeMap;
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/recovery-raw-state.TEST_ONLY.json")).unwrap()
}
#[test]
fn raw_state_semantics_normalization_and_manifests_match_all_actual_source_opaque_cases() {
    let fixture = fixture();
    let now = Timestamp::parse(fixture["now"].as_str().unwrap()).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let raw = STANDARD
            .decode(case["payload_json_base64"].as_str().unwrap())
            .unwrap();
        let parsed = parse_state_json(
            PythonJson::parse(&raw).unwrap(),
            now,
            &TimeZone::utc(),
            &BTreeMap::new(),
            &|| false,
        )
        .unwrap_or_else(|_| panic!("{}", case["name"]));
        assert_eq!(parsed.preview, case["preview"], "{}", case["name"]);
        assert_eq!(
            parsed.payload.canonical(&|| false).unwrap(),
            STANDARD
                .decode(case["canonical_payload_base64"].as_str().unwrap())
                .unwrap(),
            "{}",
            case["name"]
        );
    }
}
#[test]
fn actual_source_v1_cipher_decrypts_and_reencodes_without_losing_opaque_json() {
    let fixture = fixture();
    let bundle = STANDARD
        .decode(fixture["bundle_base64"].as_str().unwrap())
        .unwrap();
    let secret = fixture["password"].as_str().unwrap();
    let payload = decode_json_bundle(&bundle, secret).unwrap();
    let expected = STANDARD
        .decode(
            fixture["cases"][0]["canonical_payload_base64"]
                .as_str()
                .unwrap(),
        )
        .unwrap();
    assert_eq!(payload.canonical(&|| false).unwrap(), expected);
    let encoded = encode_json_bundle(payload, secret).unwrap();
    assert_eq!(
        decode_json_bundle(&encoded, secret)
            .unwrap()
            .canonical(&|| false)
            .unwrap(),
        expected
    );
}
