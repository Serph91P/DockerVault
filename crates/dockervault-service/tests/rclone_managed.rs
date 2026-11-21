use dockervault_core::{cron::TimeZone, crypto::CredentialCipher, timestamp::Timestamp};
use dockervault_service::recovery_state::{ValidationContext, validate_state_json};
use dockervault_service::{ownership::Owner, process::ProcessOwner, rclone::ManagedConfigBuilder};
use std::{path::PathBuf, sync::Arc};

#[tokio::test]
async fn failed_or_cancelled_preparation_never_touches_an_existing_generation() {
    let root = std::env::temp_dir().join(format!("dv-managed-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let generation = root.join("generation");
    std::fs::create_dir(&generation).unwrap();
    std::fs::write(generation.join("foreign"), b"PUBLIC foreign sentinel").unwrap();
    let processes = ProcessOwner::new(1, env!("CARGO_BIN_EXE_dockervault-process-guard")).unwrap();
    let blocking = Owner::new(1).unwrap();
    let tasks = Owner::new(1).unwrap();
    let fixture: serde_json::Value = serde_json::from_str(include_str!(
        "../../dockervault-core/tests/fixtures/recovery-state.TEST_ONLY.json"
    ))
    .unwrap();
    let raw = serde_json::to_vec(&fixture["cases"][0]["payload"]).unwrap();
    let state = Arc::new(
        validate_state_json(
            raw,
            ValidationContext {
                backup_root: root.join("backups"),
                now: Timestamp::parse("2024-01-01T00:00:00").unwrap(),
                timezone: TimeZone::utc(),
                rclone_options: serde_json::from_value(fixture["options"].clone()).unwrap(),
            },
            &blocking,
        )
        .await
        .unwrap(),
    );
    let builder = ManagedConfigBuilder::new(
        processes.clone(),
        blocking.clone(),
        tasks.clone(),
        PathBuf::from("/bin/false"),
    );
    let cipher = Arc::new(CredentialCipher::new(&CredentialCipher::generate_key()).unwrap());
    let result = builder
        .start(state.clone(), generation.clone(), cipher.clone())
        .unwrap()
        .wait()
        .await
        .unwrap();
    assert!(result.is_err());
    assert_eq!(
        std::fs::read(generation.join("foreign")).unwrap(),
        b"PUBLIC foreign sentinel"
    );
    let fresh = root.join("fresh");
    assert!(
        builder
            .start(state.clone(), fresh.clone(), cipher.clone())
            .unwrap()
            .wait()
            .await
            .unwrap()
            .is_err()
    );
    assert!(!fresh.exists());
    let cli = root.join("fake-cli");
    use std::os::unix::fs::PermissionsExt;
    std::fs::write(
        &cli,
        b"#!/bin/sh\ncase \" $* \" in *' rcd '*) exit 3;; esac\nexit 0\n",
    )
    .unwrap();
    std::fs::set_permissions(&cli, std::fs::Permissions::from_mode(0o700)).unwrap();
    let builder = ManagedConfigBuilder::new(
        processes.clone(),
        blocking.clone(),
        tasks.clone(),
        cli.clone(),
    );
    let dead = root.join("dead-daemon");
    assert_eq!(
        builder
            .start(state.clone(), dead.clone(), cipher.clone())
            .unwrap()
            .wait()
            .await
            .unwrap(),
        Err(dockervault_core::rclone_package::RcloneError::Invalid)
    );
    assert!(!dead.exists());
    std::fs::write(&cli,b"#!/bin/sh\ncase \" $* \" in *' rcd '*) printf '%s' \"$$\" > \"$(dirname \"$0\")/pid\"; exec /bin/sleep 60;; esac\nexit 0\n").unwrap();
    let interrupted = root.join("interrupted");
    let work = builder.start(state, interrupted.clone(), cipher).unwrap();
    let pid = tokio::time::timeout(std::time::Duration::from_secs(3), async {
        loop {
            if let Ok(raw) = std::fs::read_to_string(root.join("pid"))
                && let Ok(pid) = raw.parse::<i32>()
            {
                break pid;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert_eq!(processes.active(), 1);
    work.cancel();
    assert_eq!(
        work.wait().await.unwrap(),
        Err(dockervault_core::rclone_package::RcloneError::Cancelled)
    );
    assert!(!interrupted.exists());
    assert_eq!(
        nix::sys::signal::kill(nix::unistd::Pid::from_raw(pid), None),
        Err(nix::errno::Errno::ESRCH)
    );
    assert_eq!(processes.active(), 0);
    assert_eq!(blocking.active(), 0);
    assert_eq!(tasks.active(), 0);
    processes.shutdown(None).await;
    blocking.shutdown(None).await;
    tasks.shutdown(None).await;
    std::fs::remove_dir_all(root).unwrap();
}
