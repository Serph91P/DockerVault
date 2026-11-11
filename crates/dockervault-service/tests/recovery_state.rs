#![cfg(target_os = "linux")]
use dockervault_core::{cron::TimeZone, timestamp::Timestamp};
use dockervault_service::{
    ownership::Owner,
    recovery_state::{ValidationContext, validate_state},
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    os::unix::fs::symlink,
};
struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new(index: usize) -> Self {
        let path = std::env::temp_dir().join(format!("dv-state-{}-{index}", std::process::id()));
        std::fs::create_dir(&path).unwrap();
        Self(path)
    }
    fn path(&self) -> &std::path::Path {
        &self.0
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        std::fs::remove_dir_all(&self.0).unwrap();
    }
}
#[tokio::test]
async fn state_validation_refuses_a_closed_physical_owner_without_touching_files() {
    use dockervault_service::recovery_records::RecordValidationError;
    let owner = Owner::new(1).unwrap();
    owner.shutdown(None).await;
    let fixture: Value = serde_json::from_str(include_str!(
        "../../dockervault-core/tests/fixtures/recovery-state.TEST_ONLY.json"
    ))
    .unwrap();
    let context = ValidationContext {
        backup_root: std::env::temp_dir().join("PUBLIC-unopened-root"),
        now: Timestamp::parse(fixture["now"].as_str().unwrap()).unwrap(),
        timezone: TimeZone::utc(),
        rclone_options: serde_json::from_value(fixture["options"].clone()).unwrap(),
    };
    assert_eq!(
        validate_state(fixture["cases"][0]["payload"].clone(), context, &owner)
            .await
            .err(),
        Some(RecordValidationError::Owner)
    );
}
#[tokio::test]
async fn full_offline_state_validation_matches_actual_provider_and_normalizes_public_custody() {
    let fixture: Value = serde_json::from_str(include_str!(
        "../../dockervault-core/tests/fixtures/recovery-state.TEST_ONLY.json"
    ))
    .unwrap();
    let options: BTreeMap<String, BTreeSet<String>> =
        serde_json::from_value(fixture["options"].clone()).unwrap();
    let owner = Owner::new(1).unwrap();
    for (index, case) in fixture["cases"].as_array().unwrap().iter().enumerate() {
        let work = Fixture::new(index);
        let root = work.path().join("destination");
        let outside = work.path().join("outside");
        std::fs::create_dir(&root).unwrap();
        std::fs::create_dir(&outside).unwrap();
        match case["filesystem"].as_str().unwrap() {
            "root-symlink" => {
                std::fs::remove_dir(&root).unwrap();
                symlink(&outside, &root).unwrap();
            }
            "parent-symlink" => {
                symlink(&outside, root.join("link")).unwrap();
            }
            "leaf-symlink" => {
                symlink(outside.join("absent"), root.join("archive.tar")).unwrap();
            }
            "file-parent" => {
                std::fs::write(root.join("plain"), b"PUBLIC").unwrap();
            }
            "missing-root" => {
                std::fs::remove_dir(&root).unwrap();
            }
            "file-root" => {
                std::fs::remove_dir(&root).unwrap();
                std::fs::write(&root, b"PUBLIC").unwrap();
            }
            "fifo-leaf" => {
                nix::unistd::mkfifo(
                    &root.join("archive.tar"),
                    nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
                )
                .unwrap();
            }
            "none" => {}
            _ => panic!("unknown fixture"),
        }
        let context = ValidationContext {
            backup_root: root.clone(),
            now: Timestamp::parse(fixture["now"].as_str().unwrap()).unwrap(),
            timezone: TimeZone::utc(),
            rclone_options: options.clone(),
        };
        let result = validate_state(case["payload"].clone(), context, &owner).await;
        assert_eq!(
            result.is_ok(),
            case["accepted"].as_bool().unwrap(),
            "{}",
            case["name"]
        );
        if let Ok(state) = result {
            assert_eq!(state.preview(), &case["preview"], "{}", case["name"]);
            assert_eq!(
                state.payload()["ssh_public_keys"],
                case["normalized_ssh"],
                "{}",
                case["name"]
            );
            assert_eq!(
                state.payload()["manifest"],
                case["normalized_manifest"],
                "{}",
                case["name"]
            );
        }
        assert!(std::fs::read_dir(&outside).unwrap().next().is_none());
        if case["filesystem"] == "file-parent" {
            assert_eq!(std::fs::read(root.join("plain")).unwrap(), b"PUBLIC");
        }
    }
    owner.shutdown(None).await;
}
