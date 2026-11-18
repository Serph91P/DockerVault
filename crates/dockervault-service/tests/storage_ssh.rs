use dockervault_service::{
    ownership::Owner,
    process::{ProcessError, ProcessOwner},
    storage::ssh::{SftpEndpoint, remote_path},
};

#[tokio::test]
async fn unsafe_destination_is_refused_before_admitting_a_process() {
    let processes = ProcessOwner::new(1, env!("CARGO_BIN_EXE_dockervault-process-guard")).unwrap();
    for (host, username) in [
        ("-ProxyCommand", "backup"),
        ("host/path", "backup"),
        ("backup.example", "other@backup"),
    ] {
        let endpoint = SftpEndpoint {
            host: host.into(),
            username: username.into(),
            port: 22,
            base: "/backups".into(),
            known_hosts: "/app/data/ssh_known_hosts".into(),
            identity: None,
            password: None,
        };
        assert!(matches!(
            endpoint.probe(&processes),
            Err(ProcessError::InvalidSpec)
        ));
        assert_eq!(processes.active(), 0);
    }
}

#[tokio::test]
async fn unsafe_download_is_rejected_without_replacing_destination() {
    let processes = ProcessOwner::new(1, env!("CARGO_BIN_EXE_dockervault-process-guard")).unwrap();
    let workers = Owner::new(1).unwrap();
    let dir = std::env::temp_dir().join(format!("dockervault-sftp-{}", std::process::id()));
    std::fs::create_dir(&dir).unwrap();
    let existing = dir.join("existing.tar.gz");
    std::fs::write(&existing, b"previous archive").unwrap();
    let endpoint = SftpEndpoint {
        host: "127.0.0.1".into(),
        username: "backup".into(),
        port: 22,
        base: "/backups".into(),
        known_hosts: dir.join("known_hosts"),
        identity: None,
        password: None,
    };
    let result = endpoint.download(
        &processes,
        &workers,
        "../unsafe",
        std::fs::File::open(&dir).unwrap(),
        "existing.tar.gz".into(),
    );
    let rsync_result = endpoint.download_rsync(
        &processes,
        &workers,
        "../unsafe",
        std::fs::File::open(&dir).unwrap(),
        "existing.tar.gz".into(),
    );
    assert!(rsync_result.is_err());
    let untouched = std::fs::read(&existing).unwrap();
    std::fs::remove_dir_all(&dir).unwrap();
    assert!(result.is_err());
    assert_eq!(processes.active(), 0);
    assert_eq!(workers.active(), 0);
    assert_eq!(untouched, b"previous archive");
}

#[tokio::test]
async fn unsafe_upload_is_rejected_before_mutation_or_admission() {
    use dockervault_service::storage::VerifiedSource;
    use std::sync::Arc;

    let processes = ProcessOwner::new(1, env!("CARGO_BIN_EXE_dockervault-process-guard")).unwrap();
    let workers = Owner::new(1).unwrap();
    let source_path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("Cargo.toml");
    let source = Arc::new(VerifiedSource::open(&source_path, &|| false).unwrap());
    let endpoint = SftpEndpoint {
        host: "127.0.0.1".into(),
        username: "backup".into(),
        port: 22,
        base: "/backups".into(),
        known_hosts: "/app/data/ssh_known_hosts".into(),
        identity: None,
        password: None,
    };
    let scratch = std::fs::File::open(std::env::temp_dir()).unwrap();
    assert!(
        endpoint
            .upload(&processes, &workers, "sub/../a.key", source, scratch)
            .is_err()
    );
    assert_eq!((processes.active(), workers.active()), (0, 0));
}

#[tokio::test]
async fn unsafe_delete_never_starts_an_sftp_session() {
    let processes = ProcessOwner::new(1, env!("CARGO_BIN_EXE_dockervault-process-guard")).unwrap();
    let workers = Owner::new(1).unwrap();
    let endpoint = SftpEndpoint {
        host: "127.0.0.1".into(),
        username: "backup".into(),
        port: 22,
        base: "/backups".into(),
        known_hosts: "/app/data/ssh_known_hosts".into(),
        identity: None,
        password: None,
    };
    for child in ["", "../a.key", "dir/..", "dir/.", "dir/", "bad\nname"] {
        assert!(
            endpoint.delete(&processes, &workers, child).is_err(),
            "{child:?}"
        );
    }
    assert_eq!((processes.active(), workers.active()), (0, 0));
}

#[test]
fn storage_box_maps_paths_only_on_port_23() {
    for (host, port, base, child, expected) in [
        (
            "sub.Your-StorageBox.de",
            23,
            " /home/ ",
            "/archive.tar.gz",
            "archive.tar.gz",
        ),
        (
            "sub.your-storagebox.de",
            23,
            "/home/archives/",
            "a.key",
            "archives/a.key",
        ),
        ("sub.your-storagebox.de", 23, "/backups", "", "."),
        (
            "sub.your-storagebox.de",
            22,
            "/backups",
            "a.key",
            "/backups/a.key",
        ),
        ("backup.example", 23, "/", "a.key", "/a.key"),
        (
            "backup.example",
            22,
            "/data set",
            "daily files/a.key",
            "/data set/daily files/a.key",
        ),
    ] {
        assert_eq!(remote_path(host, port, base, child).unwrap(), expected);
    }
}

#[test]
fn remote_children_cannot_escape_the_configured_base() {
    for child in ["../a.key", "child/../../a.key", "bad\nname", "bad\0name"] {
        assert!(
            remote_path("backup.example", 22, "/backups", child).is_err(),
            "{child:?}"
        );
    }
}
