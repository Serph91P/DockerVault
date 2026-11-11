use base64::{Engine, engine::general_purpose::STANDARD};
use dockervault_core::lossless_json::PythonJson;
use serde_json::Value;
#[test]
fn observed_json_format_cancellation_never_returns_partial_output_or_invalid() {
    use dockervault_core::lossless_json::JsonError;
    let raw = format!("{{\"PUBLIC\":{}0{}}}", "[".repeat(2048), "]".repeat(2048));
    let json = PythonJson::parse(raw.as_bytes()).unwrap();
    assert_eq!(json.canonical(&|| true).err(), Some(JsonError::Cancelled));
    assert_eq!(json.python(&|| true).err(), Some(JsonError::Cancelled));
    for canonical in [false, true] {
        for at in 1..=32 {
            let calls = std::cell::Cell::new(0);
            let raised = std::cell::Cell::new(false);
            let cancel = || {
                calls.set(calls.get() + 1);
                let now = calls.get() == at;
                if now {
                    raised.set(true);
                }
                now
            };
            let result = if canonical {
                json.canonical(&cancel).map(|_| ())
            } else {
                json.python(&cancel).map(|_| ())
            };
            if raised.get() {
                assert_eq!(result, Err(JsonError::Cancelled));
            } else {
                assert!(result.is_ok());
            }
        }
    }
}
#[test]
fn lossless_raw_json_matches_actual_source_canonical_and_sql_text_without_surrogate_substitution() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/lossless-json.TEST_ONLY.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let raw = STANDARD
            .decode(case["input_base64"].as_str().unwrap())
            .unwrap();
        let actual = PythonJson::parse(&raw)
            .and_then(|j| Ok((j.canonical(&|| false)?, j.python(&|| false)?)));
        assert_eq!(
            actual.is_ok(),
            case["accepted"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
        if let Ok((canonical, python)) = actual {
            assert_eq!(
                canonical,
                STANDARD
                    .decode(case["canonical_base64"].as_str().unwrap())
                    .unwrap(),
                "{}",
                case["name"]
            );
            assert_eq!(
                python.as_bytes(),
                STANDARD
                    .decode(case["python_base64"].as_str().unwrap())
                    .unwrap(),
                "{}",
                case["name"]
            );
        }
    }
}
