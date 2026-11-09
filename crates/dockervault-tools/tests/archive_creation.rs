use dockervault_tools::archive_worker::{Request, create_archive};
use serde_json::json;
use std::{
    collections::BTreeMap,
    fs::{self, File},
    io::Read,
    os::unix::fs::{PermissionsExt, symlink},
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
};

static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(PathBuf);
impl Fixture {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "dv-archive-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        for name in ["source", "backups", "outside"] {
            fs::create_dir(path.join(name)).unwrap();
        }
        Self(path)
    }
    fn request(&self, compress: bool) -> Request {
        Request::parse(&serde_json::to_vec(&json!({"version":1,"dest":"/backups/result.tar","compress":compress,"sources":[["root",self.0.join("source")]],"allowed_sources":[self.0.join("source")]})).unwrap()).unwrap()
    }
    fn backups(&self) -> File {
        File::open(self.0.join("backups")).unwrap()
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}

#[test]
fn plain_and_gzip6_preserve_data_modes_links_and_never_follow_directory_symlinks() {
    for compressed in [false, true] {
        let root = Fixture::new();
        fs::write(root.0.join("source/data"), b"PUBLIC archive bytes\0\n").unwrap();
        fs::set_permissions(
            root.0.join("source/data"),
            fs::Permissions::from_mode(0o640),
        )
        .unwrap();
        fs::hard_link(root.0.join("source/data"), root.0.join("source/hard")).unwrap();
        fs::write(
            root.0.join("outside/marker"),
            b"PUBLIC must not be followed",
        )
        .unwrap();
        symlink(root.0.join("outside"), root.0.join("source/link-dir")).unwrap();
        let report = create_archive(&root.request(compressed), &root.backups(), &|| false).unwrap();
        assert!(report.skipped_files.is_empty());
        let file = File::open(root.0.join("backups/result.tar")).unwrap();
        let reader: Box<dyn Read> = if compressed {
            Box::new(flate2::read::GzDecoder::new(file))
        } else {
            Box::new(file)
        };
        let mut archive = tar::Archive::new(reader);
        let mut entries = BTreeMap::new();
        for entry in archive.entries().unwrap() {
            let mut entry = entry.unwrap();
            let name = entry
                .path()
                .unwrap()
                .to_string_lossy()
                .trim_end_matches('/')
                .to_owned();
            let kind = entry.header().entry_type();
            let mode = entry.header().mode().unwrap();
            let link = entry.link_name().unwrap().map(|p| p.into_owned());
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            entries.insert(name, (kind, mode, link, bytes));
        }
        assert_eq!(entries.len(), 4);
        assert!(entries["root"].0.is_dir());
        assert!(entries["root/link-dir"].0.is_symlink());
        let regular = ["root/data", "root/hard"]
            .into_iter()
            .find(|name| entries[*name].0.is_file())
            .unwrap();
        let hard = ["root/data", "root/hard"]
            .into_iter()
            .find(|name| entries[*name].0.is_hard_link())
            .unwrap();
        assert_eq!(entries[regular].3, b"PUBLIC archive bytes\0\n");
        assert_eq!(entries[regular].1 & 0o777, 0o640);
        assert_eq!(
            entries[hard].2.as_deref(),
            Some(std::path::Path::new(regular))
        );
    }
}

#[test]
fn existing_output_cancel_and_symlinked_destination_never_mutate_another_recovery_point() {
    let root = Fixture::new();
    let output = root.0.join("backups/result.tar");
    fs::write(&output, b"PUBLIC recovery point").unwrap();
    assert!(create_archive(&root.request(false), &root.backups(), &|| false).is_err());
    assert_eq!(fs::read(&output).unwrap(), b"PUBLIC recovery point");
    fs::remove_file(&output).unwrap();
    assert!(create_archive(&root.request(false), &root.backups(), &|| true).is_err());
    assert!(!output.exists());
    symlink(root.0.join("outside"), root.0.join("backups/sub")).unwrap();
    let mut request = root.request(false);
    request.dest = "/backups/sub/result.tar".into();
    assert!(create_archive(&request, &root.backups(), &|| false).is_err());
    assert_eq!(fs::read_dir(root.0.join("outside")).unwrap().count(), 0);
}

#[test]
fn symlink_fractional_negative_time_is_preserved_in_pax_without_dereference() {
    let root = Fixture::new();
    let source = root.0.join("source/link");
    symlink("does-not-exist", &source).unwrap();
    let time = nix::sys::time::TimeSpec::new(-1, 500_000_000);
    nix::sys::stat::utimensat(
        nix::fcntl::AT_FDCWD,
        &source,
        &time,
        &time,
        nix::sys::stat::UtimensatFlags::NoFollowSymlink,
    )
    .unwrap();
    create_archive(&root.request(false), &root.backups(), &|| false).unwrap();
    let mut archive = tar::Archive::new(File::open(root.0.join("backups/result.tar")).unwrap());
    let mut found = false;
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if entry.path().unwrap() == std::path::Path::new("root/link") {
            let values: Vec<_> = entry
                .pax_extensions()
                .unwrap()
                .unwrap()
                .map(|v| {
                    let v = v.unwrap();
                    (v.key_bytes().to_vec(), v.value_bytes().to_vec())
                })
                .collect();
            assert!(
                values
                    .iter()
                    .any(|(k, v)| k == b"mtime" && v == b"-0.500000000")
            );
            found = true;
        }
    }
    assert!(found);
}

#[test]
fn long_unicode_paths_fractional_mtime_and_pax_remain_standard_tar() {
    let root = Fixture::new();
    let name = format!("{}-{}.txt", "ä".repeat(60), "x".repeat(60));
    let source = root.0.join("source").join(&name);
    fs::write(&source, b"PUBLIC PAX bytes").unwrap();
    let modified = std::time::UNIX_EPOCH + std::time::Duration::new(1_234_567_890, 250_000_000);
    File::open(&source)
        .unwrap()
        .set_times(std::fs::FileTimes::new().set_modified(modified))
        .unwrap();
    create_archive(&root.request(false), &root.backups(), &|| false).unwrap();
    let mut archive = tar::Archive::new(File::open(root.0.join("backups/result.tar")).unwrap());
    let mut found = false;
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if entry.path().unwrap() == PathBuf::from("root").join(&name) {
            let extensions: Vec<_> = entry
                .pax_extensions()
                .unwrap()
                .unwrap()
                .map(|v| {
                    let v = v.unwrap();
                    (v.key_bytes().to_vec(), v.value_bytes().to_vec())
                })
                .collect();
            assert!(
                extensions
                    .iter()
                    .any(|(k, v)| k == b"mtime" && v == b"1234567890.250000000")
            );
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, b"PUBLIC PAX bytes");
            found = true;
        }
    }
    assert!(found);
    if let Some(export) = std::env::var_os("DV248_EXPORT_DIR") {
        fs::create_dir_all(&export).unwrap();
        fs::copy(
            root.0.join("backups/result.tar"),
            PathBuf::from(export).join("native-pax.TEST_ONLY.tar"),
        )
        .unwrap();
    }
}

#[test]
fn non_utf8_filesystem_names_roundtrip_without_replacement_character_collisions() {
    use std::os::unix::ffi::{OsStrExt, OsStringExt};
    let root = Fixture::new();
    let name = std::ffi::OsString::from_vec(vec![b'x', 0xff]);
    fs::write(root.0.join("source").join(&name), b"PUBLIC byte filename").unwrap();
    create_archive(&root.request(false), &root.backups(), &|| false).unwrap();
    let mut archive = tar::Archive::new(File::open(root.0.join("backups/result.tar")).unwrap());
    let mut found = false;
    for entry in archive.entries().unwrap() {
        let mut entry = entry.unwrap();
        if entry.path().unwrap().as_os_str().as_bytes()
            == [b'r', b'o', b'o', b't', b'/', b'x', 0xff]
        {
            let mut bytes = Vec::new();
            entry.read_to_end(&mut bytes).unwrap();
            assert_eq!(bytes, b"PUBLIC byte filename");
            found = true;
        }
    }
    assert!(found);
}

#[test]
fn selected_nested_files_keep_ancestor_directories_and_excluded_subtrees_are_pruned() {
    let root = Fixture::new();
    fs::create_dir_all(root.0.join("source/data/deep")).unwrap();
    fs::create_dir_all(root.0.join("source/data/excluded")).unwrap();
    fs::write(root.0.join("source/data/deep/keep.txt"), b"PUBLIC keep").unwrap();
    fs::write(
        root.0.join("source/data/excluded/other.txt"),
        b"PUBLIC excluded",
    )
    .unwrap();
    let request = Request::parse(&serde_json::to_vec(&json!({"version":1,"dest":"/backups/result.tar","compress":false,"sources":[["root",root.0.join("source")]],"allowed_sources":[root.0.join("source")],"include_paths":["data/deep/keep.txt","data/excluded/*"],"exclude_paths":["data/excluded"]})).unwrap()).unwrap();
    create_archive(&request, &root.backups(), &|| false).unwrap();
    let mut archive = tar::Archive::new(File::open(root.0.join("backups/result.tar")).unwrap());
    let paths: Vec<_> = archive
        .entries()
        .unwrap()
        .map(|e| e.unwrap().path().unwrap().into_owned())
        .collect();
    assert!(paths.contains(&PathBuf::from("root/data/deep/keep.txt")));
    assert!(paths.contains(&PathBuf::from("root/data")));
    assert!(!paths.iter().any(|p| p.starts_with("root/data/excluded")));
}

#[test]
fn unreadable_files_are_reported_without_corrupting_the_remaining_tar() {
    let root = Fixture::new();
    let denied = root.0.join("source/denied");
    fs::write(&denied, b"PUBLIC unavailable").unwrap();
    fs::set_permissions(&denied, fs::Permissions::from_mode(0o000)).unwrap();
    let report = create_archive(&root.request(false), &root.backups(), &|| false).unwrap();
    assert_eq!(report.skipped_files, vec![denied.clone()]);
    fs::set_permissions(denied, fs::Permissions::from_mode(0o600)).unwrap();
    let mut archive = tar::Archive::new(File::open(root.0.join("backups/result.tar")).unwrap());
    assert_eq!(archive.entries().unwrap().count(), 1);
}

#[test]
fn cancellation_during_copy_settles_and_removes_all_owned_private_staging() {
    let root = Fixture::new();
    let file = File::create(root.0.join("source/large")).unwrap();
    file.set_len(3 * 1024 * 1024).unwrap();
    let calls = AtomicU64::new(0);
    assert!(
        create_archive(&root.request(false), &root.backups(), &|| calls
            .fetch_add(1, Ordering::Relaxed)
            > 5)
        .is_err()
    );
    assert_eq!(fs::read_dir(root.0.join("backups")).unwrap().count(), 0);
}

#[test]
fn multi_source_volume_rules_override_globals_and_keep_logical_roots() {
    let root = Fixture::new();
    for name in ["keep", "drop"] {
        fs::write(root.0.join("source").join(name), name).unwrap();
    }
    let request = Request::parse(&serde_json::to_vec(&json!({"version":1,"dest":"/backups/result.tar","compress":false,"sources":[["stack/data",root.0.join("source")],["other",root.0.join("source")]],"allowed_sources":[root.0.join("source")],"include_paths":["drop"],"per_volume_rules":{"data":{"include_paths":["keep"]}}})).unwrap()).unwrap();
    create_archive(&request, &root.backups(), &|| false).unwrap();
    let mut archive = tar::Archive::new(File::open(root.0.join("backups/result.tar")).unwrap());
    let paths: Vec<_> = archive
        .entries()
        .unwrap()
        .map(|e| e.unwrap().path().unwrap().into_owned())
        .collect();
    assert!(paths.contains(&PathBuf::from("stack/data/keep")));
    assert!(paths.contains(&PathBuf::from("other/drop")));
    assert!(!paths.contains(&PathBuf::from("stack/data/drop")));
    assert!(!paths.contains(&PathBuf::from("other/keep")));
}
