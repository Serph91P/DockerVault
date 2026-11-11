use base64::{Engine, engine::general_purpose::STANDARD};
use dockervault_core::ssh_public::canonical_public_key;
use serde_json::Value;

#[test]
fn accepted_python_public_material_canonicalization_never_preserves_following_private_data() {
    for fixture in [
        include_str!("fixtures/ssh-public.TEST_ONLY.json"),
        include_str!("fixtures/ssh-public-adverse.TEST_ONLY.json"),
        include_str!("fixtures/ssh-public-boundary.TEST_ONLY.json"),
        include_str!("fixtures/ssh-public-certificates.TEST_ONLY.json"),
        include_str!("fixtures/ssh-public-cert-adverse.TEST_ONLY.json"),
    ] {
        let fixture: Value = serde_json::from_str(fixture).unwrap();
        assert_eq!(
            fixture["source"],
            "fc1b3392fec6ecf9562ef88875ab143858d637bf"
        );
        for vector in fixture["vectors"].as_array().unwrap() {
            let input = STANDARD
                .decode(vector["raw_base64"].as_str().unwrap())
                .unwrap();
            let actual = canonical_public_key(&input);
            if let Some(expected) = vector["expected"].as_str() {
                let actual = actual.unwrap_or_else(|_| panic!("{} rejected", vector["name"]));
                assert_eq!(actual, expected, "{}", vector["name"]);
                assert!(!actual.contains("PRIVATE_"));
                assert_eq!(canonical_public_key(actual.as_bytes()).unwrap(), actual);
            } else {
                assert!(actual.is_err(), "{} accepted", vector["name"]);
            }
        }
    }
}

#[test]
fn hostile_embedded_lengths_fail_without_trusting_the_declared_allocation() {
    for algorithm in [
        "ssh-rsa",
        "ssh-ed25519",
        "ecdsa-sha2-nistp256",
        "ssh-ed25519-cert-v01@openssh.com",
    ] {
        let mut wire = Vec::new();
        wire.extend_from_slice(&(algorithm.len() as u32).to_be_bytes());
        wire.extend_from_slice(algorithm.as_bytes());
        wire.extend_from_slice(&u32::MAX.to_be_bytes());
        let input = format!("{algorithm} {}", STANDARD.encode(wire));
        assert!(
            canonical_public_key(input.as_bytes()).is_err(),
            "{algorithm}"
        );
    }
}

#[test]
fn malformed_public_material_returns_only_a_generic_error() {
    for raw in [
        b"PRIVATE_TEST_PAYLOAD".as_slice(),
        b"ssh-ed25519 PRIVATE_TEST_PAYLOAD",
    ] {
        let error = canonical_public_key(raw).unwrap_err();
        assert!(!error.to_string().contains("PRIVATE_TEST_PAYLOAD"));
        assert!(!format!("{error:?}").contains("PRIVATE_TEST_PAYLOAD"));
    }
}
