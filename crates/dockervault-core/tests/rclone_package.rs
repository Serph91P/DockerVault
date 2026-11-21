use dockervault_core::{
    portable_records::RemoteStorage,
    rclone_package::{RcloneError, validate},
};
use serde_json::Value;
use std::collections::{BTreeMap, BTreeSet};
#[test]
fn canceled_validation_cannot_be_misreported_as_a_credentials_required_success() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/rclone-packages.TEST_ONLY.json")).unwrap();
    let options: BTreeMap<String, BTreeSet<String>> =
        serde_json::from_value(fixture["options"].clone()).unwrap();
    let case = fixture["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|v| v["name"] == "missing")
        .unwrap();
    let storages: Vec<RemoteStorage> = serde_json::from_value(case["storages"].clone()).unwrap();
    assert_eq!(
        validate(&case["payload"], &storages, &options, &|| true).err(),
        Some(RcloneError::Cancelled)
    );
    for interrupt_at in 1..=16 {
        let calls = std::cell::Cell::new(0);
        let raised = std::cell::Cell::new(false);
        let interrupt = || {
            let call = calls.get() + 1;
            calls.set(call);
            let now = call == interrupt_at;
            if now {
                raised.set(true);
            }
            now
        };
        let result = validate(&case["payload"], &storages, &options, &interrupt);
        if raised.get() {
            assert_eq!(result.err(), Some(RcloneError::Cancelled));
        } else {
            assert!(result.is_ok());
        }
    }
    assert!(validate(&case["payload"], &storages, &options, &|| false).is_ok());
}
#[test]
fn portable_rclone_packages_match_the_actual_provider_import_validator_without_host_access() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/rclone-packages.TEST_ONLY.json")).unwrap();
    let options: BTreeMap<String, BTreeSet<String>> =
        serde_json::from_value(fixture["options"].clone()).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let storages: Vec<RemoteStorage> =
            serde_json::from_value(case["storages"].clone()).unwrap();
        assert_eq!(
            validate(&case["payload"], &storages, &options, &|| false).is_ok(),
            case["accepted"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
    }
}
