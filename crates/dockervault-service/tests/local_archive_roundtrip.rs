//! Real native archive -> optional codec -> independently verified LOCAL copies -> mutation -> restore.
//! Component integration only: this does not claim HTTP/jobs, process restart or S3 coverage.
use dockervault_core::crypto::{decrypt_dek, decrypt_file, decrypt_private, encrypt_file};
use dockervault_service::{
    ownership::Owner,
    restore::IndexedArchive,
    storage::{VerifiedSource, local::LocalStorage},
};
use dockervault_tools::archive_worker::{
    Request, archive_private, create_archive, create_archive_from_directory,
};
#[path = "../../dockervault-core/tests/common/mod.rs"]
mod common;
use std::{
    fs,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    path::PathBuf,
    sync::{
        Arc,
        atomic::{AtomicU64, Ordering},
    },
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dv-native-roundtrip-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        for name in ["source", "backups", "remote", "downloads"] {
            fs::create_dir(path.join(name)).unwrap();
        }
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
#[tokio::test]
async fn authenticated_decryption_returns_only_an_anonymous_completed_snapshot() {
    use nix::{
        fcntl::{OFlag, openat},
        sys::stat::Mode,
    };
    use std::io::Read;
    let fixture = Fixture::new();
    let root = fs::File::open(fixture.0.join("backups")).unwrap();
    let anonymous = || {
        fs::File::from(
            openat(
                &root,
                ".",
                OFlag::O_TMPFILE | OFlag::O_RDWR | OFlag::O_CLOEXEC,
                Mode::S_IRUSR | Mode::S_IWUSR,
            )
            .unwrap(),
        )
    };
    let material = decrypt_dek(
        &include_bytes!("../../dockervault-core/tests/fixtures/pax-links.tar.key.TEST_ONLY")[..],
        &common::public_identity(false),
    )
    .unwrap();
    let good = include_bytes!("../../dockervault-core/tests/fixtures/pax-links.tar.enc");
    let bad = include_bytes!("../../dockervault-core/tests/fixtures/pax-links.tar.tampered.enc");
    assert!(
        decrypt_private(&bad[..], bad.len() as u64, anonymous(), &material, &|| {
            false
        })
        .is_err()
    );
    let mut plaintext = decrypt_private(
        &good[..],
        good.len() as u64,
        anonymous(),
        &material,
        &|| false,
    )
    .unwrap();
    let mut bytes = Vec::new();
    plaintext.read_to_end(&mut bytes).unwrap();
    assert_eq!(
        bytes,
        include_bytes!("../../dockervault-core/tests/fixtures/pax-links.tar")
    );
    let named = fixture.0.join("backups/named");
    fs::write(&named, b"").unwrap();
    let file = fs::OpenOptions::new()
        .read(true)
        .write(true)
        .open(&named)
        .unwrap();
    assert!(decrypt_private(&good[..], good.len() as u64, file, &material, &|| false).is_err());
    assert_eq!(fs::metadata(named).unwrap().len(), 0);
}

#[tokio::test]
async fn pinned_path_archive_reads_the_claimed_directory_not_its_replacement() {
    let root = Fixture::new();
    let source = root.0.join("source");
    fs::write(source.join("data"), b"claimed bytes").unwrap();
    let pinned = fs::File::open(&source).unwrap();
    fs::rename(&source, root.0.join("moved")).unwrap();
    fs::create_dir(&source).unwrap();
    fs::write(source.join("data"), b"replacement bytes").unwrap();
    let request = Request::parse(
        &serde_json::to_vec(&serde_json::json!({
            "version":1,"dest":"/backups/archive.tar","compress":false,"sources":[["source",source]]
        }))
        .unwrap(),
    )
    .unwrap();
    let backup_dir = root.0.join("backups");
    let backups = fs::File::open(&backup_dir).unwrap();
    let anonymous = archive_private(&request, &pinned, &backups, &|| false).unwrap();
    assert_eq!(anonymous.metadata().unwrap().nlink(), 0);
    assert_eq!(fs::read_dir(&backup_dir).unwrap().count(), 0);
    assert!(
        tar::Archive::new(anonymous)
            .entries()
            .unwrap()
            .any(|entry| entry.unwrap().path().unwrap() == std::path::Path::new("source/data"))
    );
    assert!(create_archive_from_directory(&request, &pinned, &backups, &|| true).is_err());
    assert_eq!(fs::read_dir(&backup_dir).unwrap().count(), 0);
    create_archive_from_directory(&request, &pinned, &backups, &|| false).unwrap();
    let file = fs::File::open(backup_dir.join("archive.tar")).unwrap();
    let mut archive = tar::Archive::new(file);
    let mut data = Vec::new();
    use std::io::Read;
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if entry.path().unwrap() == std::path::Path::new("source/data") {
            entry.read_to_end(&mut data).unwrap();
        }
    }
    assert_eq!(data, b"claimed bytes");
}

#[tokio::test]
async fn plain_and_encrypted_compressed_and_uncompressed_restore_real_mutated_files() {
    for compressed in [false, true] {
        for encrypted in [false, true] {
            let root = Fixture::new();
            let source = root.0.join("source");
            let backups = root.0.join("backups");
            let payload: Vec<_> = (0..1024 * 1024 + 17).map(|n| (n % 251) as u8).collect();
            fs::write(source.join("data"), &payload).unwrap();
            fs::set_permissions(source.join("data"), fs::Permissions::from_mode(0o640)).unwrap();
            fs::File::open(source.join("data"))
                .unwrap()
                .set_modified(
                    std::time::UNIX_EPOCH + std::time::Duration::new(1_234_567_890, 250_000_000),
                )
                .unwrap();
            fs::hard_link(source.join("data"), source.join("hard")).unwrap();
            symlink("data", source.join("link")).unwrap();
            let request=Request::parse(&serde_json::to_vec(&serde_json::json!({"version":1,"dest":"/backups/archive.tar","compress":compressed,"sources":[["root",source]],"allowed_sources":[source]})).unwrap()).unwrap();
            assert_eq!(
                create_archive(&request, &fs::File::open(&backups).unwrap(), &|| false)
                    .unwrap()
                    .skipped_count(),
                0
            );
            let archive = backups.join("archive.tar");
            let cipher = backups.join("archive.enc");
            let key_path = backups.join("archive.key");
            let sidecar =
                include_bytes!("../../dockervault-core/tests/fixtures/pax-links.tar.key.TEST_ONLY");
            let identity = common::public_identity(false);
            let material = decrypt_dek(sidecar.as_slice(), &identity).unwrap();
            let upload = if encrypted {
                encrypt_file(&archive, &cipher, &material, &|| false).unwrap();
                fs::write(&key_path, sidecar).unwrap();
                &cipher
            } else {
                &archive
            };
            let owner = Owner::new(2).unwrap();
            let backend = LocalStorage::new(
                fs::File::open(root.0.join("remote")).unwrap(),
                owner.clone(),
            )
            .unwrap();
            let source_receipt = Arc::new(VerifiedSource::open(upload, &|| false).unwrap());
            backend
                .upload(source_receipt.clone(), "archive".into())
                .unwrap()
                .wait()
                .await
                .unwrap()
                .unwrap();
            if encrypted {
                backend
                    .upload(
                        Arc::new(VerifiedSource::open(&key_path, &|| false).unwrap()),
                        "archive.key".into(),
                    )
                    .unwrap()
                    .wait()
                    .await
                    .unwrap()
                    .unwrap();
            }
            let expected_hash = source_receipt.sha256_hex();
            drop(source_receipt);
            fs::remove_file(&archive).unwrap();
            if encrypted {
                fs::remove_file(&cipher).unwrap();
                fs::remove_file(&key_path).unwrap();
            }
            fs::write(source.join("data"), b"mutated data").unwrap();
            fs::write(source.join("unrelated"), b"must survive overlay").unwrap();
            let download = root.0.join("downloads");
            let remote_receipt = backend
                .download(
                    "archive".into(),
                    fs::File::open(&download).unwrap(),
                    "archive".into(),
                )
                .unwrap()
                .wait()
                .await
                .unwrap()
                .unwrap();
            assert_eq!(remote_receipt.sha256_hex(), expected_hash);
            let prepared = if encrypted {
                backend
                    .download(
                        "archive.key".into(),
                        fs::File::open(&download).unwrap(),
                        "archive.key".into(),
                    )
                    .unwrap()
                    .wait()
                    .await
                    .unwrap()
                    .unwrap();
                let recovered = decrypt_dek(
                    &fs::read(download.join("archive.key")).unwrap()[..],
                    &identity,
                )
                .unwrap();
                decrypt_file(
                    &download.join("archive"),
                    &download.join("plain"),
                    &recovered,
                    &|| false,
                )
                .unwrap();
                download.join("plain")
            } else {
                download.join("archive")
            };
            let mut index = IndexedArchive::prepare(
                fs::File::open(prepared).unwrap(),
                compressed,
                &fs::File::open(&backups).unwrap(),
                Some(b"root"),
                &|| false,
            )
            .unwrap();
            index
                .restore_into(&fs::File::open(&source).unwrap(), &|| false)
                .unwrap();
            assert_eq!(fs::read(source.join("data")).unwrap(), payload);
            assert_eq!(fs::read(source.join("hard")).unwrap(), payload);
            let metadata = fs::metadata(source.join("data")).unwrap();
            assert_eq!(
                metadata.ino(),
                fs::metadata(source.join("hard")).unwrap().ino()
            );
            assert_eq!(metadata.mode() & 0o7777, 0o640);
            assert_eq!(
                (metadata.mtime(), metadata.mtime_nsec()),
                (1_234_567_890, 250_000_000)
            );
            assert_eq!(
                fs::read_link(source.join("link")).unwrap(),
                std::path::Path::new("data")
            );
            assert_eq!(
                fs::read(source.join("unrelated")).unwrap(),
                b"must survive overlay"
            );
            assert_eq!(owner.active(), 0);
            assert_eq!(fs::read_dir(backups).unwrap().count(), 0);
        }
    }
}
