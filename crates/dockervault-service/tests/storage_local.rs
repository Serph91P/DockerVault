use dockervault_service::{
    ownership::Owner,
    storage::{VerifiedSource, local::LocalStorage},
};
use std::{
    fs,
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
        let p = std::env::temp_dir().join(format!(
            "dv-local-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn setup() -> (Fixture, LocalStorage, Owner) {
    let root = Fixture::new();
    fs::create_dir(root.0.join("remote")).unwrap();
    let owner = Owner::new(2).unwrap();
    let backend = LocalStorage::new(
        fs::File::open(root.0.join("remote")).unwrap(),
        owner.clone(),
    )
    .unwrap();
    (root, backend, owner)
}
#[tokio::test]
async fn real_local_upload_and_independent_download_preserve_bytes_mode_and_mtime() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt};
    let (root, backend, owner) = setup();
    let source_path = root.0.join("source");
    let bytes = vec![83; 1024 * 1024 + 3];
    fs::write(&source_path, &bytes).unwrap();
    fs::set_permissions(&source_path, fs::Permissions::from_mode(0o640)).unwrap();
    let source = Arc::new(VerifiedSource::open(&source_path, &|| false).unwrap());
    backend
        .upload(source.clone(), PathBuf::from("nested/archive.enc"))
        .unwrap()
        .wait()
        .await
        .unwrap()
        .unwrap();
    let remote = root.0.join("remote/nested/archive.enc");
    assert_eq!(fs::read(&remote).unwrap(), bytes);
    assert_eq!(fs::metadata(&remote).unwrap().mode() & 0o777, 0o640);
    assert_eq!(
        fs::metadata(&remote).unwrap().mtime_nsec(),
        fs::metadata(&source_path).unwrap().mtime_nsec()
    );
    fs::create_dir(root.0.join("downloads")).unwrap();
    let receipt = backend
        .download(
            PathBuf::from("nested/archive.enc"),
            fs::File::open(root.0.join("downloads")).unwrap(),
            PathBuf::from("restored"),
        )
        .unwrap()
        .wait()
        .await
        .unwrap()
        .unwrap();
    assert_eq!(receipt.len(), bytes.len() as u64);
    assert_eq!(receipt.sha256_hex(), source.sha256_hex());
    assert_eq!(fs::read(root.0.join("downloads/restored")).unwrap(), bytes);
    assert_eq!(owner.active(), 0);
    assert_eq!(
        fs::read_dir(root.0.join("remote/nested")).unwrap().count(),
        1
    );
}
#[tokio::test]
async fn paths_cannot_escape_root_or_follow_an_existing_link() {
    use std::os::unix::fs::symlink;
    let (root, backend, _) = setup();
    let path = root.0.join("source");
    fs::write(&path, b"safe").unwrap();
    let source = Arc::new(VerifiedSource::open(&path, &|| false).unwrap());
    fs::create_dir(root.0.join("outside")).unwrap();
    symlink(root.0.join("outside"), root.0.join("remote/link")).unwrap();
    for path in ["../escaped", "/absolute", "link/escaped"] {
        assert!(
            backend
                .upload(source.clone(), PathBuf::from(path))
                .unwrap()
                .wait()
                .await
                .unwrap()
                .is_err()
        );
    }
    assert_eq!(fs::read_dir(root.0.join("outside")).unwrap().count(), 0);
    assert!(!root.0.join("escaped").exists());
}
#[tokio::test]
async fn cancelled_transfer_settles_worker_and_preserves_existing_destination() {
    let (root, backend, owner) = setup();
    let path = root.0.join("source");
    fs::write(&path, vec![7; 8 * 1024 * 1024]).unwrap();
    let source = Arc::new(VerifiedSource::open(&path, &|| false).unwrap());
    fs::write(root.0.join("remote/archive"), b"existing").unwrap();
    let work = backend.upload(source, PathBuf::from("archive")).unwrap();
    work.cancel();
    assert!(work.wait().await.unwrap().is_err());
    assert_eq!(owner.active(), 0);
    assert_eq!(
        fs::read(root.0.join("remote/archive")).unwrap(),
        b"existing"
    );
    assert_eq!(fs::read_dir(root.0.join("remote")).unwrap().count(), 1);
}
#[tokio::test]
async fn cancel_after_real_staging_writes_removes_partial_before_releasing_capacity() {
    use nix::sys::inotify::{AddWatchFlags, InitFlags, Inotify};
    let (root, backend, owner) = setup();
    let path = root.0.join("source");
    let file = fs::File::create(&path).unwrap();
    file.set_len(64 * 1024 * 1024).unwrap();
    drop(file);
    let source = Arc::new(VerifiedSource::open(&path, &|| false).unwrap());
    fs::write(root.0.join("remote/archive"), b"existing").unwrap();
    let events = Inotify::init(InitFlags::IN_NONBLOCK | InitFlags::IN_CLOEXEC).unwrap();
    events
        .add_watch(&root.0.join("remote"), AddWatchFlags::IN_MODIFY)
        .unwrap();
    let work = backend.upload(source, PathBuf::from("archive")).unwrap();
    let deadline = tokio::time::Instant::now() + std::time::Duration::from_secs(2);
    loop {
        let modifying = match events.read_events() {
            Ok(events) => events.iter().any(|event| {
                event.name.as_ref().is_some_and(|name| {
                    name.to_string_lossy().starts_with(".dockervault-transfer-")
                })
            }),
            Err(nix::errno::Errno::EAGAIN) => false,
            Err(error) => panic!("inotify failed: {error}"),
        };
        if modifying {
            break;
        }
        assert!(
            tokio::time::Instant::now() < deadline,
            "No actual staging write observed"
        );
        tokio::time::sleep(std::time::Duration::from_millis(1)).await;
    }
    work.cancel();
    assert!(work.wait().await.unwrap().is_err());
    assert_eq!(owner.active(), 0);
    assert_eq!(
        fs::read(root.0.join("remote/archive")).unwrap(),
        b"existing"
    );
    assert_eq!(fs::read_dir(root.0.join("remote")).unwrap().count(), 1);
}

#[tokio::test]
async fn failed_publication_preserves_an_existing_directory_and_cleans_only_owned_partial() {
    let (root, backend, owner) = setup();
    let path = root.0.join("source");
    fs::write(&path, b"bytes").unwrap();
    let source = Arc::new(VerifiedSource::open(&path, &|| false).unwrap());
    fs::create_dir(root.0.join("remote/archive")).unwrap();
    fs::write(root.0.join("remote/archive/sentinel"), b"existing").unwrap();
    assert!(
        backend
            .upload(source, PathBuf::from("archive"))
            .unwrap()
            .wait()
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(owner.active(), 0);
    assert_eq!(
        fs::read(root.0.join("remote/archive/sentinel")).unwrap(),
        b"existing"
    );
    assert_eq!(fs::read_dir(root.0.join("remote")).unwrap().count(), 1);
}

#[tokio::test]
async fn source_change_after_receipt_cannot_publish_a_verified_copy() {
    let (root, backend, owner) = setup();
    let path = root.0.join("source");
    fs::write(&path, b"before").unwrap();
    let source = Arc::new(VerifiedSource::open(&path, &|| false).unwrap());
    fs::write(&path, b"after!").unwrap();
    assert!(
        backend
            .upload(source, PathBuf::from("archive"))
            .unwrap()
            .wait()
            .await
            .unwrap()
            .is_err()
    );
    assert_eq!(owner.active(), 0);
    assert_eq!(fs::read_dir(root.0.join("remote")).unwrap().count(), 0);
}
