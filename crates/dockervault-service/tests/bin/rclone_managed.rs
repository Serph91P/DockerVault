//! Offline real pinned-rclone generation/encryption/readback proof.
use dockervault_core::{cron::TimeZone, crypto::CredentialCipher, timestamp::Timestamp};
use dockervault_service::{
    ownership::Owner,
    process::{ProcessOwner, ProcessSpec},
    rclone::ManagedConfigBuilder,
    recovery_state::{ValidationContext, validate_state},
};
use serde_json::{Value, json};
use std::{path::PathBuf, sync::Arc};
#[tokio::main]
async fn main() {
    let args: Vec<PathBuf> = std::env::args_os().skip(1).map(PathBuf::from).collect();
    assert_eq!(args.len(), 2);
    let processes = ProcessOwner::new(1, &args[0]).unwrap();
    let blocking = Owner::new(1).unwrap();
    let tasks = Owner::new(1).unwrap();
    let builder = ManagedConfigBuilder::new(
        processes.clone(),
        blocking.clone(),
        tasks.clone(),
        args[1].clone(),
    );
    let fixture: Value = serde_json::from_str(include_str!(
        "../../../dockervault-core/tests/fixtures/recovery-state.TEST_ONLY.json"
    ))
    .unwrap();
    let packages: Value = serde_json::from_str(include_str!(
        "../../../dockervault-core/tests/fixtures/rclone-packages.TEST_ONLY.json"
    ))
    .unwrap();
    let root = std::env::temp_dir().join(format!("dv-managed-actual-{}", std::process::id()));
    std::fs::create_dir(&root).unwrap();
    let cipher = Arc::new(CredentialCipher::new(&CredentialCipher::generate_key()).unwrap());
    let mut count = 0;
    for name in [
        "local",
        "s3-environment",
        "drive-file",
        "provider-oracleobjectstorage",
        "provider-cache",
        "sftp-pubkey_file",
        "shared-file-roots",
    ] {
        let mut case = packages["cases"]
            .as_array()
            .unwrap()
            .iter()
            .find(|c| c["name"] == name)
            .unwrap()
            .clone();
        if name == "sftp-pubkey_file" {
            case["payload"]["remotes"]["PUBLIC"]["pass"] =
                json!("PUBLIC literal credential; not obscured twice");
        }
        if name == "provider-oracleobjectstorage" {
            case["payload"]["remotes"]["PUBLIC"]["config_profile"] = json!("PUBLIC");
        }
        assert_eq!(case["accepted"], true);
        let mut payload = fixture["cases"][0]["payload"].clone();
        payload["rclone"] = case["payload"].clone();
        payload["tables"]["remote_storages"] = case["storages"].clone();
        payload["manifest"] = dockervault_core::recovery_state::state_manifest(&payload).unwrap();
        let state = validate_state(
            payload,
            ValidationContext {
                backup_root: root.join("backups"),
                now: Timestamp::parse(fixture["now"].as_str().unwrap()).unwrap(),
                timezone: TimeZone::utc(),
                rclone_options: serde_json::from_value(packages["options"].clone()).unwrap(),
            },
            &blocking,
        )
        .await
        .unwrap();
        let dir = root.join(format!("generation-{count}"));
        let result = builder
            .start(Arc::new(state), dir.clone(), cipher.clone())
            .unwrap()
            .wait()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(result["directory"], dir.to_str().unwrap());
        assert_eq!(result["roots"], case["payload"]["roots"]);
        assert_eq!(
            serde_json::from_str::<Value>(
                &cipher
                    .decrypt_value(result["files"].as_str().unwrap())
                    .unwrap()
            )
            .unwrap(),
            case["payload"]["files"]
        );
        let unlock = cipher
            .decrypt_value(result["unlock"].as_str().unwrap())
            .unwrap();
        assert_eq!(unlock.len(), 64);
        let raw = std::fs::read(dir.join("rclone.conf")).unwrap();
        assert!(raw.starts_with(b"# Encrypted rclone configuration File\n"));
        assert!(!dir.join("rc").exists());
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&dir).unwrap().permissions().mode() & 0o777,
            0o700
        );
        assert_eq!(
            std::fs::metadata(dir.join("rclone.conf"))
                .unwrap()
                .permissions()
                .mode()
                & 0o777,
            0o600
        );
        let mut spec = ProcessSpec::new(
            &args[1],
            [
                "--config",
                dir.join("rclone.conf").to_str().unwrap(),
                "--ask-password=false",
                "--password-command",
                "/usr/bin/printenv DV_RECOVERY_RCLONE_UNLOCK",
                "config",
                "dump",
            ],
        );
        spec.env = vec![
            ("PATH".into(), "/bin:/usr/bin:/usr/local/bin".into()),
            ("HOME".into(), "/nonexistent".into()),
            ("DV_RECOVERY_RCLONE_UNLOCK".into(), unlock.into()),
        ];
        spec.stdout_limit = 8 * 1024 * 1024;
        let out = processes
            .start(spec)
            .unwrap()
            .wait()
            .await
            .unwrap()
            .unwrap();
        assert!(out.status.success());
        let dump: Value = serde_json::from_slice(&out.stdout).unwrap();
        let mut expected = case["payload"]["remotes"].clone();
        let mut overrides = json!({});
        let mut refs = json!({});
        let mut caches = vec![];
        for (remote, fields) in expected.as_object_mut().unwrap() {
            if fields["type"] == "cache" {
                caches.push(remote.clone());
            }
            for (key, val) in fields.as_object_mut().unwrap() {
                if let Some(id) = val
                    .get("file_ref")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
                {
                    refs[remote][key] = json!(id);
                    *val = json!(format!("dv-recovery-file:{id}"));
                }
                if key.starts_with("config_") {
                    overrides[remote][key] = val.clone();
                }
            }
        }
        for fields in expected.as_object_mut().unwrap().values_mut() {
            fields
                .as_object_mut()
                .unwrap()
                .retain(|k, _| !k.starts_with("config_"));
        }
        assert_eq!(dump, expected, "{name}");
        let restored = builder
            .start_read_config(result.clone(), cipher.clone())
            .unwrap()
            .wait()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(restored, case["payload"]["remotes"], "{name}");
        let exported = builder
            .start_export_managed(
                result.clone(),
                cipher.clone(),
                serde_json::from_value(case["storages"].clone()).unwrap(),
                serde_json::from_value(packages["options"].clone()).unwrap(),
            )
            .unwrap()
            .wait()
            .await
            .unwrap()
            .unwrap();
        assert_eq!(exported, case["payload"], "{name}");
        if name == "s3-environment" {
            let remote = case["payload"]["remotes"]
                .as_object()
                .unwrap()
                .keys()
                .next()
                .unwrap();
            let mut update = ProcessSpec::new(
                &args[1],
                vec![
                    "--config".to_owned(),
                    dir.join("rclone.conf").to_str().unwrap().to_owned(),
                    "--ask-password=false".to_owned(),
                    "--password-command".to_owned(),
                    "/usr/bin/printenv DV_RECOVERY_RCLONE_UNLOCK".to_owned(),
                    "config".to_owned(),
                    "update".to_owned(),
                    remote.to_owned(),
                    "region".to_owned(),
                    "PUBLIC-updated-region".to_owned(),
                    "--non-interactive".to_owned(),
                    "--no-obscure".to_owned(),
                ],
            );
            update.env = vec![
                ("PATH".into(), "/bin:/usr/bin:/usr/local/bin".into()),
                ("HOME".into(), "/nonexistent".into()),
                (
                    "DV_RECOVERY_RCLONE_UNLOCK".into(),
                    cipher
                        .decrypt_value(result["unlock"].as_str().unwrap())
                        .unwrap()
                        .into(),
                ),
            ];
            assert!(
                processes
                    .start(update)
                    .unwrap()
                    .wait()
                    .await
                    .unwrap()
                    .unwrap()
                    .status
                    .success()
            );
            let refreshed = builder
                .start_export_managed(
                    result.clone(),
                    cipher.clone(),
                    serde_json::from_value(case["storages"].clone()).unwrap(),
                    serde_json::from_value(packages["options"].clone()).unwrap(),
                )
                .unwrap()
                .wait()
                .await
                .unwrap()
                .unwrap();
            let mut expected = case["payload"].clone();
            expected["remotes"][remote]["region"] = json!("PUBLIC-updated-region");
            assert_eq!(refreshed, expected);
            assert!(
                std::fs::read(dir.join("rclone.conf"))
                    .unwrap()
                    .starts_with(b"# Encrypted rclone configuration File\n")
            );
        }
        if name == "local" {
            let lock = std::fs::OpenOptions::new()
                .read(true)
                .write(true)
                .open(dir.join(".lock"))
                .unwrap();
            let lock =
                nix::fcntl::Flock::lock(lock, nix::fcntl::FlockArg::LockExclusiveNonblock).unwrap();
            let watch = nix::sys::inotify::Inotify::init(nix::sys::inotify::InitFlags::IN_NONBLOCK)
                .unwrap();
            watch
                .add_watch(
                    dir.join(".lock").as_path(),
                    nix::sys::inotify::AddWatchFlags::IN_OPEN,
                )
                .unwrap();
            let work = builder
                .start_read_config(result.clone(), cipher.clone())
                .unwrap();
            tokio::time::timeout(std::time::Duration::from_secs(2), async {
                loop {
                    match watch.read_events() {
                        Ok(events) if !events.is_empty() => break,
                        Ok(_) | Err(nix::errno::Errno::EAGAIN) => tokio::task::yield_now().await,
                        Err(error) => panic!("{error}"),
                    }
                }
            })
            .await
            .unwrap();
            assert_eq!(processes.active(), 0);
            work.cancel();
            assert!(matches!(
                tokio::time::timeout(std::time::Duration::from_secs(2), work.wait())
                    .await
                    .unwrap()
                    .unwrap(),
                Err(dockervault_core::rclone_package::RcloneError::Cancelled)
            ));
            assert_eq!(blocking.active(), 0);
            assert_eq!(tasks.active(), 0);
            drop(lock);
            let mut bad = result.clone();
            let remote = case["payload"]["remotes"]
                .as_object()
                .unwrap()
                .keys()
                .next()
                .unwrap();
            bad["overrides"] = json!(
                cipher.encrypt_value(&json!({(remote):{"type":"PUBLIC wrong type"}}).to_string())
            );
            assert!(
                builder
                    .start_read_config(bad, cipher.clone())
                    .unwrap()
                    .wait()
                    .await
                    .unwrap()
                    .is_err()
            );
            assert_eq!(std::fs::read(dir.join("rclone.conf")).unwrap(), raw);
            assert_eq!(
                builder
                    .start_read_config(result.clone(), cipher.clone())
                    .unwrap()
                    .wait()
                    .await
                    .unwrap()
                    .unwrap(),
                restored
            );
        }
        assert_eq!(result["file_refs"], refs);
        assert_eq!(result["cache_remotes"], json!(caches));
        let got: Value = serde_json::from_str(
            &cipher
                .decrypt_value(result["overrides"].as_str().unwrap())
                .unwrap(),
        )
        .unwrap();
        for remote in case["payload"]["remotes"].as_object().unwrap().keys() {
            if overrides.get(remote).is_none() {
                overrides[remote] = json!({});
            }
        }
        assert_eq!(got, overrides);
        assert_eq!(processes.active(), 0);
        assert_eq!(blocking.active(), 0);
        assert_eq!(tasks.active(), 0);
        count += 1;
    }
    std::fs::remove_dir_all(root).unwrap();
    processes.shutdown(None).await;
    blocking.shutdown(None).await;
    tasks.shutdown(None).await;
    println!(
        "managed-rclone: {count} real offline encrypted generations, native dump and custody checks: PASS"
    );
}
