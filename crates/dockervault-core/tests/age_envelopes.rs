//! Every identity/DEK in this fixture corpus is PUBLIC SYNTHETIC TEST MATERIAL.
use age::secrecy::ExposeSecret;
use dockervault_core::crypto::{decrypt_dek, encrypt_dek};
const ID: &str = include_str!("fixtures/pax-links.tar.identity.TEST_ONLY");
fn expected() -> Vec<u8> {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/credential-vectors.TEST_ONLY.json")).unwrap();
    data["dek_hex"]
        .as_str()
        .unwrap()
        .as_bytes()
        .as_chunks::<2>()
        .0
        .iter()
        .map(|s| u8::from_str_radix(std::str::from_utf8(s).unwrap(), 16).unwrap())
        .collect()
}
#[test]
fn original_age_cli_sidecars_recover_v1_and_legacy_binary_material() {
    for (cipher, identity, versioned) in [
        (
            include_bytes!("fixtures/pax-links.tar.key").as_slice(),
            ID,
            true,
        ),
        (
            include_bytes!("fixtures/pax-links.tar.legacy.key").as_slice(),
            ID,
            false,
        ),
        (
            include_bytes!("fixtures/pax-links.tar.gz.key").as_slice(),
            include_str!("fixtures/pax-links.tar.gz.identity.TEST_ONLY"),
            true,
        ),
        (
            include_bytes!("fixtures/pax-links.tar.gz.legacy.key").as_slice(),
            include_str!("fixtures/pax-links.tar.gz.identity.TEST_ONLY"),
            false,
        ),
    ] {
        let material = decrypt_dek(cipher, identity).unwrap();
        let mut plain = if versioned {
            b"DVKEY\0\x01\n".to_vec()
        } else {
            vec![]
        };
        plain.extend(expected());
        assert!(material.as_slice() == plain);
    }
}
#[test]
fn native_age_wraps_the_version_binding_and_exports_public_cli_oracle_sidecar() {
    let identity: age::x25519::Identity = ID.trim().parse().unwrap();
    let public = identity.to_public().to_string();
    let dek = expected();
    let armored = encrypt_dek(&dek, &public).unwrap();
    assert!(armored.starts_with(b"-----BEGIN AGE ENCRYPTED FILE-----"));
    let plain = decrypt_dek(armored.as_slice(), &format!(" \n{}\n", ID.trim())).unwrap();
    assert!(plain.starts_with(b"DVKEY\0\x01\n"));
    assert!(plain[8..] == dek);
    std::fs::write(
        std::env::temp_dir().join("native-age.TEST_ONLY.key"),
        armored,
    )
    .unwrap();
}
#[test]
fn offline_identity_files_accept_comments_and_try_every_identity_without_caching() {
    let wrong = age::x25519::Identity::generate();
    let identities = format!(
        "# PUBLIC TEST ONLY\n{}\n# second identity matches\n{}\n",
        wrong.to_string().expose_secret(),
        ID.trim()
    );
    let material = dockervault_core::crypto::decrypt_dek_file(
        include_bytes!("fixtures/pax-links.tar.key").as_slice(),
        std::io::Cursor::new(identities),
    )
    .unwrap();
    assert!(material.starts_with(b"DVKEY\0\x01\n"));
    assert!(material[8..] == expected());
}
#[test]
fn wrong_keys_tamper_truncation_and_unsupported_material_are_rejected() {
    let identity: age::x25519::Identity = ID.trim().parse().unwrap();
    let public = identity.to_public();
    assert!(encrypt_dek(&[0; 31], &public.to_string()).is_err());
    assert!(encrypt_dek(&[0; 32], "invalid").is_err());
    let cipher = encrypt_dek(&[0; 32], &public.to_string()).unwrap();
    let wrong = age::x25519::Identity::generate();
    assert!(decrypt_dek(cipher.as_slice(), wrong.to_string().expose_secret()).is_err());
    let mut tampered = cipher.clone();
    let middle = tampered.len() / 2;
    tampered[middle] = if tampered[middle] == b'A' { b'B' } else { b'A' };
    assert!(decrypt_dek(tampered.as_slice(), ID).is_err());
    assert!(decrypt_dek(&cipher[..cipher.len() / 2], ID).is_err());
    assert!(decrypt_dek(&b"invalid"[..], ID).is_err());
    for bytes in [vec![0; 31], vec![0; 41], vec![0; 40], vec![0; 1048576]] {
        let unsupported = age::encrypt_and_armor(&public, &bytes).unwrap();
        assert!(decrypt_dek(unsupported.as_bytes(), ID).is_err());
    }
}
