use dockervault_core::rclone_path::{environment_name, valid_name};
use openssl::sha::Sha256;
use serde_json::Value;
#[test]
fn environment_names_match_actual_source_for_every_valid_scalar_and_titlecase() {
    let fixture: Value =
        serde_json::from_str(include_str!("fixtures/rclone-environment.TEST_ONLY.json")).unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        assert_eq!(
            environment_name(
                case["section"].as_str().unwrap(),
                case["field"].as_str().unwrap()
            ),
            case["expected"].as_str().unwrap()
        );
    }
    let mut hash = Sha256::new();
    let mut count = 0u64;
    for cp in 0..=0x10ffffu32 {
        let Some(c) = char::from_u32(cp) else {
            continue;
        };
        let s = c.to_string();
        if !valid_name(&s) {
            continue;
        }
        hash.update(&cp.to_be_bytes());
        hash.update(environment_name(&s, "known-hosts-file").as_bytes());
        hash.update(&[0]);
        count += 1;
    }
    assert_eq!(count, fixture["scalar_count"].as_u64().unwrap());
    let digest: String = hash.finish().iter().map(|b| format!("{b:02x}")).collect();
    assert_eq!(digest, fixture["scalar_sha256"].as_str().unwrap());
}
