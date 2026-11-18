use dockervault_service::storage::ssh::remote_path;

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
