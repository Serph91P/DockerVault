use base64::{Engine, engine::general_purpose::STANDARD};
use dockervault_core::rclone_ssh::{SshMaterial, canonical_material};
use serde_json::Value;
#[test]
fn rclone_ssh_material_matches_actual_provider_without_copying_private_following_data() {
    let fixture: Value = serde_json::from_str(include_str!(
        "fixtures/rclone-public-material.TEST_ONLY.json"
    ))
    .unwrap();
    for case in fixture["cases"].as_array().unwrap() {
        let input = STANDARD
            .decode(case["raw_base64"].as_str().unwrap())
            .unwrap();
        let field = if case["field"] == "known_hosts_file" {
            SshMaterial::KnownHosts
        } else {
            SshMaterial::PublicIdentity
        };
        let actual = canonical_material(&input, field);
        if case["rejected"] == true {
            assert!(actual.is_err(), "{} {}", case["name"], case["field"]);
        } else {
            let actual = actual
                .unwrap_or_else(|_| panic!("source accepted {} {}", case["name"], case["field"]));
            assert_eq!(
                STANDARD.encode(actual),
                case["expected_base64"],
                "{} {}",
                case["name"],
                case["field"]
            );
        }
    }
}
