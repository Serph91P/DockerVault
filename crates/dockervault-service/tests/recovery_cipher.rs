use base64::{Engine, engine::general_purpose::URL_SAFE};
use dockervault_core::crypto::CredentialCipher;
use dockervault_service::{
    db::Database,
    ownership::Owner,
    recovery_activation::{ClaimKind, JournalError, KeyPreparation, PreparationClaims},
};
use std::{
    io::{Seek, SeekFrom, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::PathBuf,
};
use tokio_util::sync::CancellationToken;
fn public_key() -> String {
    URL_SAFE.encode((0u8..32).collect::<Vec<_>>())
}
async fn database(name: &str) -> PathBuf {
    let root = std::env::temp_dir().join(format!("dv-cipher-{name}-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let owner = Owner::new(1).unwrap();
    let db = Database::open(root.join("state.sqlite"), root.join("migration"), &owner)
        .await
        .unwrap();
    db.close().await;
    owner.shutdown(None).await;
    root
}
#[tokio::test]
async fn prepared_key_loading_preserves_custody_identity_and_never_rotates_or_falls_back() {
    let root = database("owned").await;
    let db = root.join("state.sqlite");
    let key = root.join("instance-key");
    let claims = PreparationClaims::new(Some(db.clone()));
    let guard = claims.try_claim(ClaimKind::Setup).unwrap();
    let generation = guard.begin_recovery().unwrap();
    let cancel = CancellationToken::new();
    assert!(matches!(
        generation.load_instance_cipher(&key, "", &cancel),
        Err(JournalError::UnsafeKeyPreparation)
    ));
    assert!(!key.exists());
    assert_eq!(
        generation
            .prepare_instance_key(&key, false, &cancel)
            .unwrap(),
        KeyPreparation::Created
    );
    let original = zeroize::Zeroizing::new(std::fs::read(&key).unwrap());
    let before = std::fs::metadata(&key).unwrap();
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
    let cipher = generation.load_instance_cipher(&key, "", &cancel).unwrap();
    assert_eq!(
        std::fs::metadata(&key).unwrap().permissions().mode() & 0o777,
        0o600
    );
    assert_eq!(std::fs::read(&key).unwrap(), *original);
    let independent = CredentialCipher::new(std::str::from_utf8(&original).unwrap()).unwrap();
    let token = cipher.encrypt_value("PUBLIC prepared credential");
    assert_eq!(
        independent.decrypt_value(&token).unwrap(),
        "PUBLIC prepared credential"
    );
    let after = std::fs::metadata(&key).unwrap();
    assert_eq!((before.dev(), before.ino()), (after.dev(), after.ino()));
    std::fs::write(&key, public_key()).unwrap();
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
    assert!(matches!(
        generation.load_instance_cipher(&key, "", &cancel),
        Err(JournalError::UnsafeKeyPreparation)
    ));
    assert_eq!(
        std::fs::metadata(&key).unwrap().permissions().mode() & 0o777,
        0o644
    );
    assert_eq!(std::fs::read_to_string(&key).unwrap(), public_key());
    std::fs::write(&key, original.as_slice()).unwrap();
    cancel.cancel();
    assert!(matches!(
        generation.load_instance_cipher(&key, "", &cancel),
        Err(JournalError::Cancelled)
    ));
    drop(generation);
    drop(guard);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn configured_key_wins_without_file_effects_and_unsafe_files_never_supply_custody() {
    let root = database("precedence").await;
    let key = root.join("instance-key");
    let claims = PreparationClaims::new(Some(root.join("state.sqlite")));
    let guard = claims.try_claim(ClaimKind::Setup).unwrap();
    let generation = guard.begin_recovery().unwrap();
    let cancel = CancellationToken::new();
    std::fs::write(&key, b"PUBLIC-invalid-file").unwrap();
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
    let token = CredentialCipher::new(&public_key())
        .unwrap()
        .encrypt_value("PUBLIC");
    assert_eq!(
        generation
            .load_instance_cipher(&key, &public_key(), &cancel)
            .unwrap()
            .decrypt_value(&token)
            .unwrap(),
        "PUBLIC"
    );
    assert_eq!(
        std::fs::metadata(&key).unwrap().permissions().mode() & 0o777,
        0o644
    );
    assert_eq!(std::fs::read(&key).unwrap(), b"PUBLIC-invalid-file");
    std::fs::write(&key, [public_key().as_bytes(), b"\xff"].concat()).unwrap();
    assert!(matches!(
        generation.load_instance_cipher(&key, "", &cancel),
        Err(JournalError::UnsafeKeyPreparation)
    ));
    assert_eq!(
        std::fs::metadata(&key).unwrap().permissions().mode() & 0o777,
        0o644
    );
    std::fs::write(&key, format!("{}😀{}\n", " ".repeat(8191), public_key())).unwrap();
    assert!(matches!(
        generation.load_instance_cipher(&key, " ", &cancel),
        Err(JournalError::UnsafeKeyPreparation)
    ));
    assert_eq!(
        generation
            .load_instance_cipher(&key, "", &cancel)
            .unwrap()
            .decrypt_value(&token)
            .unwrap(),
        "PUBLIC"
    );
    let outside = root.join("outside");
    std::fs::write(&outside, public_key()).unwrap();
    std::fs::set_permissions(&outside, std::fs::Permissions::from_mode(0o644)).unwrap();
    std::fs::remove_file(&key).unwrap();
    std::os::unix::fs::symlink(&outside, &key).unwrap();
    assert!(matches!(
        generation.load_instance_cipher(&key, "", &cancel),
        Err(JournalError::UnsafeKeyPreparation)
    ));
    assert_eq!(
        std::fs::metadata(&outside).unwrap().permissions().mode() & 0o777,
        0o644
    );
    assert_eq!(std::fs::read_to_string(&outside).unwrap(), public_key());
    std::fs::remove_file(&key).unwrap();
    nix::unistd::mkfifo(
        &key,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )
    .unwrap();
    assert!(matches!(
        generation.load_instance_cipher(&key, "", &cancel),
        Err(JournalError::UnsafeKeyPreparation)
    ));
    let connection = rusqlite::Connection::open(root.join("state.sqlite")).unwrap();
    let wrong = serde_json::json!({"state":"preparing","generation":"f".repeat(32)}).to_string();
    connection
        .execute(
            "UPDATE app_settings SET value=? WHERE key='recovery_activation'",
            [wrong],
        )
        .unwrap();
    assert!(matches!(
        generation.load_instance_cipher(&outside, &public_key(), &cancel),
        Err(JournalError::InvalidJournal)
    ));
    connection
        .execute(
            "UPDATE app_settings SET value='{}' WHERE key='recovery_activation'",
            [],
        )
        .unwrap();
    assert!(matches!(
        generation.load_instance_cipher(&outside, &public_key(), &cancel),
        Err(JournalError::InvalidJournal)
    ));
    connection.execute("INSERT INTO users(username,password_hash,is_admin,created_at) VALUES('PUBLIC-user','PUBLIC-not-a-credential',1,'2024-01-01 00:00:00.000000')",[]).unwrap();
    assert!(matches!(
        generation.load_instance_cipher(&outside, &public_key(), &cancel),
        Err(JournalError::AlreadySetup)
    ));
    drop(connection);
    drop(generation);
    drop(guard);
    std::fs::remove_dir_all(root).unwrap();
}
#[tokio::test]
async fn actual_file_access_cancel_physically_settles_without_key_or_journal_mutation() {
    let root = database("cancel").await;
    let key = root.join("instance-key");
    let mut file = std::fs::File::create(&key).unwrap();
    file.set_len(64 * 1024 * 1024).unwrap();
    file.seek(SeekFrom::End(0)).unwrap();
    file.write_all(public_key().as_bytes()).unwrap();
    file.sync_all().unwrap();
    drop(file);
    std::fs::set_permissions(&key, std::fs::Permissions::from_mode(0o644)).unwrap();
    let before = std::fs::metadata(&key).unwrap();
    let inotify =
        nix::sys::inotify::Inotify::init(nix::sys::inotify::InitFlags::IN_NONBLOCK).unwrap();
    inotify
        .add_watch(&key, nix::sys::inotify::AddWatchFlags::IN_ACCESS)
        .unwrap();
    let claims = PreparationClaims::new(Some(root.join("state.sqlite")));
    let guard = claims.try_claim(ClaimKind::Setup).unwrap();
    let owner = Owner::new(1).unwrap();
    let inside = key.clone();
    let work = owner
        .submit_blocking(move |cancel| {
            let generation = guard.begin_recovery()?;
            generation.load_instance_cipher(&inside, "", &cancel)
        })
        .unwrap();
    tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            match inotify.read_events() {
                Ok(events) if !events.is_empty() => break,
                Ok(_) => {}
                Err(nix::errno::Errno::EAGAIN) => {}
                Err(e) => panic!("{e}"),
            };
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(owner.active(), 1);
    work.cancel();
    assert!(matches!(
        work.wait().await.unwrap(),
        Err(JournalError::Cancelled)
    ));
    assert_eq!(owner.active(), 0);
    let after = std::fs::metadata(&key).unwrap();
    assert_eq!(
        (before.dev(), before.ino(), before.len()),
        (after.dev(), after.ino(), after.len())
    );
    assert_eq!(after.permissions().mode() & 0o777, 0o644);
    let connection = rusqlite::Connection::open(root.join("state.sqlite")).unwrap();
    let value: String = connection
        .query_row(
            "SELECT value FROM app_settings WHERE key='recovery_activation'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    let record: serde_json::Value = serde_json::from_str(&value).unwrap();
    assert_eq!(record["state"], "preparing");
    assert!(record.get("instance_key_preparing").is_none());
    drop(connection);
    owner.shutdown(None).await;
    std::fs::remove_dir_all(root).unwrap();
}
