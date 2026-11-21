use dockervault_core::storage_validation::storage_errors;
use serde_json::Value;
#[test]
fn legacy_provider_configuration_hints_match_the_actual_accepted_source() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/storage-validation.TEST_ONLY.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        assert_eq!(
            serde_json::to_value(storage_errors(&case["values"])).unwrap(),
            case["errors"],
            "{}",
            case["name"]
        );
    }
}
