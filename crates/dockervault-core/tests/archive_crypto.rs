use dockervault_core::crypto::{CryptoError, decrypt_file, encrypt_file, key_sidecar_path};
use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "dv-codec-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
fn key(versioned: bool) -> Vec<u8> {
    let data: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/credential-vectors.TEST_ONLY.json")).unwrap();
    let mut key = if versioned {
        b"DVKEY\0\x01\n".to_vec()
    } else {
        vec![]
    };
    key.extend(
        data["dek_hex"]
            .as_str()
            .unwrap()
            .as_bytes()
            .as_chunks::<2>()
            .0
            .iter()
            .map(|pair| u8::from_str_radix(std::str::from_utf8(pair).unwrap(), 16).unwrap()),
    );
    key
}
#[test]
fn original_python_v1_and_explicit_legacy_archive_bytes_decrypt_exactly() {
    let root = Fixture::new();
    let output = root.0.join("output");
    for (name, plain) in [
        (
            "pax-links.tar",
            include_bytes!("fixtures/pax-links.tar").as_slice(),
        ),
        (
            "pax-links.tar.gz",
            include_bytes!("fixtures/pax-links.tar.gz").as_slice(),
        ),
    ] {
        let base = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures");
        for versioned in [true, false] {
            decrypt_file(
                &base.join(format!("{name}.enc")),
                &output,
                &key(versioned),
                &|| false,
            )
            .unwrap();
            assert_eq!(std::fs::read(&output).unwrap(), plain);
        }
        decrypt_file(
            &base.join(format!("{name}.legacy.enc")),
            &output,
            &key(false),
            &|| false,
        )
        .unwrap();
        assert_eq!(std::fs::read(&output).unwrap(), plain);
        std::fs::write(&output, b"PUBLIC sentinel").unwrap();
        assert!(
            decrypt_file(
                &base.join(format!("{name}.legacy.enc")),
                &output,
                &key(true),
                &|| false
            )
            .is_err()
        );
        assert_eq!(std::fs::read(&output).unwrap(), b"PUBLIC sentinel");
    }
}
#[test]
fn malformed_ciphertext_and_key_fail_without_publishing_or_leaving_staging() {
    let root = Fixture::new();
    let source = root.0.join("input");
    let output = root.0.join("output");
    let valid = include_bytes!("fixtures/pax-links.tar.enc");
    let mut variants = vec![
        vec![],
        valid[..7].to_vec(),
        valid[8..].to_vec(),
        valid[..valid.len() - 1].to_vec(),
        valid.to_vec(),
    ];
    variants.last_mut().unwrap().push(0);
    variants.push(include_bytes!("fixtures/pax-links.tar.tampered.enc").to_vec());
    variants.push(include_bytes!("fixtures/pax-links.tar.truncated.enc").to_vec());
    for data in variants {
        std::fs::write(&source, data).unwrap();
        std::fs::write(&output, b"PUBLIC sentinel").unwrap();
        assert!(decrypt_file(&source, &output, &key(true), &|| false).is_err());
        assert_eq!(std::fs::read(&output).unwrap(), b"PUBLIC sentinel");
        assert_eq!(std::fs::read_dir(&root.0).unwrap().count(), 2);
    }
    std::fs::write(&source, valid).unwrap();
    assert!(decrypt_file(&source, &output, &[0; 32], &|| false).is_err());
    assert_eq!(std::fs::read(&output).unwrap(), b"PUBLIC sentinel");
}
#[test]
fn native_writes_v1_for_binary_keys_and_padding_boundaries_with_private_output() {
    use std::os::unix::fs::PermissionsExt;
    let root = Fixture::new();
    let source = root.0.join("source");
    let cipher = root.0.join("cipher");
    let output = root.0.join("output");
    for material in [key(true), vec![0; 32], vec![b'\n'; 32]] {
        for length in [0, 1, 15, 16, 17, 1048579] {
            let bytes = (0..length).map(|n| (n % 251) as u8).collect::<Vec<_>>();
            std::fs::write(&source, &bytes).unwrap();
            encrypt_file(&source, &cipher, &material, &|| false).unwrap();
            assert!(
                std::fs::read(&cipher)
                    .unwrap()
                    .starts_with(b"DVENC\0\x01\nSalted__")
            );
            decrypt_file(&cipher, &output, &material, &|| false).unwrap();
            assert_eq!(std::fs::read(&output).unwrap(), bytes);
            assert_eq!(
                std::fs::metadata(&output).unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
}
#[test]
fn cancellation_preserves_existing_destination_and_removes_only_private_staging() {
    let root = Fixture::new();
    let source = root.0.join("source");
    let output = root.0.join("output");
    std::fs::write(&source, vec![37; 3145728]).unwrap();
    std::fs::write(&output, b"PUBLIC sentinel").unwrap();
    let calls = AtomicU64::new(0);
    let cancel = || calls.fetch_add(1, Ordering::SeqCst) >= 4;
    assert_eq!(
        encrypt_file(&source, &output, &key(true), &cancel),
        Err(CryptoError::Cancelled)
    );
    assert_eq!(std::fs::read(&output).unwrap(), b"PUBLIC sentinel");
    assert_eq!(std::fs::read_dir(&root.0).unwrap().count(), 2);
}
#[test]
fn cancelled_decryption_never_publishes_its_partially_decoded_plaintext() {
    let root = Fixture::new();
    let source = root.0.join("source");
    let cipher = root.0.join("cipher");
    let output = root.0.join("output");
    std::fs::write(&source, vec![37; 3145728]).unwrap();
    encrypt_file(&source, &cipher, &key(true), &|| false).unwrap();
    std::fs::write(&output, b"PUBLIC sentinel").unwrap();
    let calls = AtomicU64::new(0);
    assert_eq!(
        decrypt_file(&cipher, &output, &key(true), &|| calls
            .fetch_add(1, Ordering::SeqCst)
            >= 3),
        Err(CryptoError::Cancelled)
    );
    assert_eq!(std::fs::read(&output).unwrap(), b"PUBLIC sentinel");
    assert_eq!(std::fs::read_dir(&root.0).unwrap().count(), 3);
}
#[test]
fn cancellation_after_private_output_sync_still_prevents_publication() {
    let root = Fixture::new();
    let source = root.0.join("source");
    let output = root.0.join("output");
    std::fs::write(&source, b"PUBLIC payload").unwrap();
    std::fs::write(&output, b"PUBLIC sentinel").unwrap();
    let calls = AtomicU64::new(0);
    assert_eq!(
        encrypt_file(&source, &output, &key(true), &|| calls
            .fetch_add(1, Ordering::SeqCst)
            >= 4),
        Err(CryptoError::Cancelled)
    );
    assert_eq!(std::fs::read(&output).unwrap(), b"PUBLIC sentinel");
    assert_eq!(std::fs::read_dir(&root.0).unwrap().count(), 2);
}
#[test]
fn terminal_only_sidecar_naming_and_native_python_oracle_export() {
    for (path, expected) in [
        ("/dir.enc/data.tar.enc", "/dir.enc/data.tar.key"),
        ("name.enc.gz", "name.enc.gz.key"),
        (".enc", ".key"),
    ] {
        assert_eq!(key_sidecar_path(path), expected);
    }
    let source = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/pax-links.tar.gz");
    encrypt_file(
        &source,
        &std::env::temp_dir().join("native-v1.TEST_ONLY.enc"),
        &key(true),
        &|| false,
    )
    .unwrap();
}
