use dockervault_core::rclone_path::{parse, valid_name};
use serde_json::Value;
fn controls() -> Value {
    serde_json::from_str(include_str!("fixtures/rclone-paths.TEST_ONLY.json")).unwrap()
}
#[test]
fn rclone_reference_parser_matches_actual_accepted_provider_connection_strings() {
    for case in controls()["cases"].as_array().unwrap() {
        let input = case["input"].as_str().unwrap();
        let result = parse(input);
        if case["rejected"] == true {
            assert!(result.is_err(), "unexpected acceptance {input:?}");
        } else {
            let parsed = result.unwrap_or_else(|_| panic!("source accepted {input:?}"));
            assert_eq!(
                serde_json::to_value(parsed).unwrap(),
                case["parsed"],
                "{input:?}"
            );
        }
    }
}
#[test]
fn all_unicode_scalar_name_characters_match_the_accepted_python_unicode_version() {
    let fixture = controls();
    let ranges: Vec<(u32, u32)> = fixture["single_character_name_ranges"]
        .as_array()
        .unwrap()
        .iter()
        .map(|r| (r[0].as_u64().unwrap() as u32, r[1].as_u64().unwrap() as u32))
        .collect();
    for code in 0..=0x10ffff {
        if let Some(c) = char::from_u32(code) {
            let index = ranges.partition_point(|r| r.0 <= code);
            let expected = index > 0 && ranges[index - 1].1 >= code;
            let mut text = [0; 4];
            assert_eq!(
                valid_name(c.encode_utf8(&mut text)),
                expected,
                "U+{code:04X}"
            );
        }
    }
}
#[test]
fn invalid_rclone_reference_errors_do_not_echo_inline_credentials() {
    let error = parse("PUBLIC,password='PRIVATE_TEST_ONLY:path")
        .err()
        .expect("unterminated override");
    assert!(!format!("{error:?} {error}").contains("PRIVATE_TEST_ONLY"));
}
