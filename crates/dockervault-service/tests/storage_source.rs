use dockervault_service::storage::VerifiedSource;
use std::{
    fs,
    io::Read,
    os::unix::fs::symlink,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dv-source-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
#[test]
fn invocation_digest_has_standard_sha256_and_replayable_full_bytes() {
    let dir = Fixture::new();
    let path = dir.0.join("source");
    fs::write(&path, b"abc").unwrap();
    let source = VerifiedSource::open(&path, &|| false).unwrap();
    assert_eq!(
        source.sha256_hex(),
        "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad"
    );
    assert_eq!(source.len(), 3);
    assert!(!source.is_empty());
    for _ in 0..3 {
        let mut bytes = Vec::new();
        source.reopen().unwrap().read_to_end(&mut bytes).unwrap();
        assert_eq!(bytes, b"abc");
    }
    source.check_unchanged().unwrap();
}
#[test]
fn rename_replacement_of_same_length_and_bytes_does_not_reuse_an_old_digest() {
    let dir = Fixture::new();
    let path = dir.0.join("source");
    fs::write(&path, b"abc").unwrap();
    let source = VerifiedSource::open(&path, &|| false).unwrap();
    let replacement = dir.0.join("replacement");
    fs::write(&replacement, b"abc").unwrap();
    fs::rename(replacement, &path).unwrap();
    assert!(source.check_unchanged().is_err());
    assert!(source.reopen().is_err());
}
#[test]
fn same_inode_mutation_cannot_hide_by_restoring_mtime() {
    let dir = Fixture::new();
    let path = dir.0.join("source");
    fs::write(&path, b"abc").unwrap();
    let modified = fs::metadata(&path).unwrap().modified().unwrap();
    let source = VerifiedSource::open(&path, &|| false).unwrap();
    fs::write(&path, b"xyz").unwrap();
    fs::File::options()
        .write(true)
        .open(&path)
        .unwrap()
        .set_modified(modified)
        .unwrap();
    assert!(source.check_unchanged().is_err());
}
#[test]
fn final_symlink_and_directory_are_not_accepted_as_owned_source_bytes() {
    let dir = Fixture::new();
    let path = dir.0.join("source");
    fs::write(&path, b"abc").unwrap();
    let alias = dir.0.join("alias");
    symlink(&path, &alias).unwrap();
    assert!(VerifiedSource::open(&alias, &|| false).is_err());
    assert!(VerifiedSource::open(&dir.0, &|| false).is_err());
}
#[test]
fn every_download_is_read_completely_and_wrong_truncated_or_extra_bytes_fail() {
    let dir = Fixture::new();
    let path = dir.0.join("source");
    fs::write(&path, b"abc").unwrap();
    let source = VerifiedSource::open(&path, &|| false).unwrap();
    for bad in [b"ab".as_slice(), b"abd", b"abcd", b""] {
        assert!(
            source
                .verify_download(&mut std::io::Cursor::new(bad), &|| false)
                .is_err()
        );
    }
    let mut remote = std::io::Cursor::new(b"abc".to_vec());
    source.verify_download(&mut remote, &|| false).unwrap();
    assert_eq!(remote.position(), 3);
}
#[test]
fn independent_download_failure_and_cancel_do_not_become_verified_success() {
    let dir = Fixture::new();
    let path = dir.0.join("source");
    fs::write(&path, b"abc").unwrap();
    let source = VerifiedSource::open(&path, &|| false).unwrap();
    struct FailedRead;
    impl Read for FailedRead {
        fn read(&mut self, _: &mut [u8]) -> std::io::Result<usize> {
            Err(std::io::Error::other("protocol closed early"))
        }
    }
    assert!(source.verify_download(&mut FailedRead, &|| false).is_err());
    assert!(
        matches!(source.verify_download(&mut std::io::Cursor::new(b"abc"),&||true),Err(e) if e.kind()==std::io::ErrorKind::Interrupted)
    );
}
#[test]
fn cancellation_during_a_real_source_hash_does_not_produce_a_receipt() {
    let dir = Fixture::new();
    let path = dir.0.join("source");
    fs::write(&path, vec![7; 1024 * 1024]).unwrap();
    let calls = std::cell::Cell::new(0);
    let result = VerifiedSource::open(&path, &|| {
        calls.set(calls.get() + 1);
        calls.get() > 2
    });
    assert!(matches!(result,Err(ref e) if e.kind()==std::io::ErrorKind::Interrupted));
}
