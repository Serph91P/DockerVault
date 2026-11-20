use base64::{Engine, engine::general_purpose::STANDARD};
use dockervault_core::{
    cron::TimeZone,
    lossless_json::PythonJson,
    recovery_bundle::{decode_json_bundle, encode_json_bundle},
    recovery_state::parse_state_json,
    timestamp::Timestamp,
};
use serde_json::{Value, value::RawValue};
use std::collections::BTreeMap;
fn fixture() -> Value {
    serde_json::from_str(include_str!("fixtures/recovery-raw-state.TEST_ONLY.json")).unwrap()
}
#[test]
fn authenticates_retired_settings_before_excluding_them_from_native_recovery() {
    use dockervault_core::recovery_state::{StateError, parse_state, state_manifest};
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/recovery-state.TEST_ONLY.json")).unwrap();
    let mut legacy = fixture["cases"][0]["payload"].clone();
    let original_settings = legacy["tables"]["app_settings"].clone();
    for (id, key) in [
        "komodo_enabled",
        "komodo_api_url",
        "komodo_api_key",
        "komodo_api_secret",
    ]
    .into_iter()
    .enumerate()
    {
        legacy["tables"]["app_settings"].as_array_mut().unwrap().push(serde_json::json!({"id":900+id,"key":key,"value":"PUBLIC synthetic retired value","updated_at":null}));
    }
    legacy["manifest"] = state_manifest(&legacy).unwrap();
    let parse = |payload| {
        parse_state(
            payload,
            Timestamp::parse(fixture["now"].as_str().unwrap()).unwrap(),
            &TimeZone::utc(),
            &BTreeMap::new(),
            &|| false,
        )
    };
    let parsed = parse(legacy.clone()).unwrap();
    let native: Value = serde_json::from_str(parsed.payload.as_str()).unwrap();
    assert_eq!(native["tables"]["app_settings"], original_settings);
    assert_eq!(native["manifest"], state_manifest(&native).unwrap());
    assert_eq!(parsed.preview, fixture["cases"][0]["preview"]);
    assert!(parse(native).is_ok());
    let mut tampered = legacy.clone();
    tampered["tables"]["app_settings"]
        .as_array_mut()
        .unwrap()
        .last_mut()
        .unwrap()["value"] = serde_json::json!("tampered retired value");
    assert_eq!(parse(tampered).err(), Some(StateError::Invalid));
    legacy["tables"]["app_settings"]
        .as_array_mut()
        .unwrap()
        .last_mut()
        .unwrap()["key"] = serde_json::json!("komodo_unrecognized");
    legacy["manifest"] = state_manifest(&legacy).unwrap();
    assert_eq!(parse(legacy).err(), Some(StateError::Invalid));
}

#[test]
fn export_manifest_hashes_opaque_catalog_json_like_python() {
    use dockervault_core::recovery_state::{StateError, state_manifest_json};
    let fixture = fixture();
    for case in fixture["cases"].as_array().unwrap() {
        let raw = STANDARD
            .decode(case["payload_json_base64"].as_str().unwrap())
            .unwrap();
        let mut fields: BTreeMap<String, Box<RawValue>> = serde_json::from_slice(&raw).unwrap();
        let expected: Value =
            serde_json::from_str(fields.remove("manifest").unwrap().get()).unwrap();
        let unsealed = PythonJson::parse(&serde_json::to_vec(&fields).unwrap()).unwrap();
        assert_eq!(state_manifest_json(&unsealed, &|| false).unwrap(), expected);
        assert_eq!(
            state_manifest_json(&unsealed, &|| true),
            Err(StateError::Cancelled)
        );
    }
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
