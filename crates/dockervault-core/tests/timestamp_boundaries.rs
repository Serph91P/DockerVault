use dockervault_core::timestamp::Timestamp;
use serde_json::Value;
#[test]
fn native_timestamp_matches_independent_python_iso_and_sql_boundary_controls() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/timestamp-boundaries.TEST_ONLY.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let input = case["input"].as_str().unwrap();
        let actual = Timestamp::parse(input);
        if case["rejected"] == true {
            assert!(actual.is_err(), "unexpected acceptance {input:?}");
        } else {
            let actual = actual.unwrap_or_else(|_| panic!("rejected accepted source {input:?}"));
            assert_eq!(actual.api(), case["api"].as_str().unwrap(), "API {input:?}");
            assert_eq!(
                actual.sql(),
                case["sqlite"].as_str().unwrap(),
                "SQL {input:?}"
            );
        }
    }
}
