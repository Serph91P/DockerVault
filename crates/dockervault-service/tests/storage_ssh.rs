use dockervault_service::{
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
