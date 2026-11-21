//! Public test vectors must remain reproducible without credential-shaped source literals.
use std::path::Path;
mod common;
#[test]
fn numeric_public_age_recipes_reconstruct_the_exact_frozen_identity_bytes() {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/age-identity-recipes.TEST_ONLY.json")).unwrap();
    for compressed in [false, true] {
        let identity = common::public_identity(compressed);
        let digest: String = openssl::sha::sha256(identity.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect();
        assert_eq!(
            digest,
            data["recipes"][usize::from(compressed)]["original_serialized_sha256"]
                .as_str()
                .unwrap()
        );
        let _: age::x25519::Identity = identity.trim().parse().unwrap();
    }
}
#[test]
fn public_age_fixtures_do_not_store_serialized_private_identity_literals() {
    let fixtures = Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
    for name in [
        "pax-links.tar.identity.TEST_ONLY",
        "pax-links.tar.gz.identity.TEST_ONLY",
    ] {
        if let Ok(bytes) = std::fs::read(fixtures.join(name)) {
            assert!(
                !bytes
                    .windows(b"AGE-SECRET-KEY-".len())
                    .any(|part| part == b"AGE-SECRET-KEY-"),
                "public synthetic fixture {name} must use the explicitly labelled raw test-vector recipe"
            );
        }
    }
}
