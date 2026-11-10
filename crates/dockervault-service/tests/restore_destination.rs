use dockervault_service::restore::IndexedArchive;
use std::{
    fs,
    io::Cursor,
    os::unix::fs::{MetadataExt, PermissionsExt, symlink},
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let root = std::env::temp_dir().join(format!(
            "dv-restore-destination-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&root).unwrap();
        fs::create_dir(root.join("target")).unwrap();
        Self(root)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn archive(entries: &serde_json::Value) -> Vec<u8> {
    let mut tar = tar::Builder::new(Vec::new());
    for entry in entries.as_array().unwrap() {
        let kind = entry[0].as_str().unwrap();
        let name = entry[1].as_str().unwrap();
        let value = entry[2].as_str().unwrap();
        let mut h = tar::Header::new_ustar();
        h.set_mode(0o640);
        h.set_mtime(0);
        h.set_size(if kind == "file" {
            value.len() as u64
        } else {
            0
        });
        h.set_entry_type(match kind {
            "file" => tar::EntryType::Regular,
            "dir" => tar::EntryType::Directory,
            "sym" => tar::EntryType::Symlink,
            "hard" => tar::EntryType::Link,
            _ => panic!("unknown fixture kind"),
        });
        if matches!(kind, "sym" | "hard") {
            h.set_link_name(value).unwrap();
        }
        h.set_cksum();
        tar.append_pax_extensions([("path", name.as_bytes())])
            .unwrap();
        tar.append_data(
            &mut h,
            "base",
            if kind == "file" {
                value.as_bytes()
            } else {
                b""
            },
        )
        .unwrap();
    }
    tar.into_inner().unwrap()
}
#[test]
fn complete_destination_effects_match_independent_python_controls_without_partial_errors() {
    let corpus: serde_json::Value =
        serde_json::from_str(include_str!("fixtures/restore-semantics.TEST_ONLY.json")).unwrap();
    for case in corpus["results"].as_array().unwrap() {
        let root = Fixture::new();
        let target = root.0.join("target");
        let mut index = IndexedArchive::prepare(
            Cursor::new(archive(&case["entries"])),
            false,
            &fs::File::open(&root.0).unwrap(),
            Some(b"root"),
            &|| false,
        )
        .unwrap();
        let result = index.restore_into(&fs::File::open(&target).unwrap(), &|| false);
        if !case["error"].is_null() {
            assert!(result.is_err(), "{}", case["case"]);
            assert_eq!(
                fs::read_dir(&target).unwrap().count(),
                0,
                "{}",
                case["case"]
            );
            continue;
        }
        result.unwrap();
        for (name, expected) in case["paths"].as_object().unwrap() {
            let path = target.join(name);
            match expected["kind"].as_str().unwrap() {
                "symlink" => assert_eq!(
                    fs::read_link(&path).unwrap().as_os_str(),
                    std::ffi::OsStr::new(expected["target"].as_str().unwrap())
                ),
                "directory" => assert!(path.is_dir()),
                "file" => {
                    assert_eq!(
                        fs::read_to_string(&path).unwrap(),
                        expected["data"].as_str().unwrap()
                    );
                    assert_eq!(
                        fs::metadata(&path).unwrap().permissions().mode() & 0o7777,
                        u32::from_str_radix(
                            expected["mode"].as_str().unwrap().trim_start_matches("0o"),
                            8
                        )
                        .unwrap()
                    );
                }
                _ => panic!("unknown expected kind"),
            }
        }
        let mut actual = std::collections::BTreeSet::new();
        let mut pending = vec![target.clone()];
        while let Some(directory) = pending.pop() {
            for entry in fs::read_dir(directory).unwrap() {
                let entry = entry.unwrap();
                let path = entry.path();
                actual.insert(
                    path.strip_prefix(&target)
                        .unwrap()
                        .to_str()
                        .unwrap()
                        .to_owned(),
                );
                if entry.file_type().unwrap().is_dir() {
                    pending.push(path);
                }
            }
        }
        let expected: std::collections::BTreeSet<_> =
            case["paths"].as_object().unwrap().keys().cloned().collect();
        assert_eq!(actual, expected, "{}", case["case"]);
        for group in case["hardlink_groups"].as_array().unwrap() {
            let paths = group.as_array().unwrap();
            let inode = fs::metadata(target.join(paths[0].as_str().unwrap()))
                .unwrap()
                .ino();
            for path in paths {
                assert_eq!(
                    fs::metadata(target.join(path.as_str().unwrap()))
                        .unwrap()
                        .ino(),
                    inode
                );
            }
        }
    }
}
#[test]
fn hardlink_to_same_path_preserves_the_existing_file_and_inode() {
    let root = Fixture::new();
    let target = root.0.join("target");
    let entries = serde_json::json!([
        ["file", "root/file", "payload"],
        ["hard", "root/file", "root/file"],
        ["hard", "root/alias", "root/file"]
    ]);
    let mut index = IndexedArchive::prepare(
        Cursor::new(archive(&entries)),
        false,
        &fs::File::open(&root.0).unwrap(),
        Some(b"root"),
        &|| false,
    )
    .unwrap();
    index
        .restore_into(&fs::File::open(&target).unwrap(), &|| false)
        .unwrap();
    assert_eq!(fs::read(target.join("file")).unwrap(), b"payload");
    assert_eq!(
        fs::metadata(target.join("file")).unwrap().ino(),
        fs::metadata(target.join("alias")).unwrap().ino()
    );
}
#[test]
fn cancellation_during_complete_plan_validation_leaves_target_unchanged() {
    let root = Fixture::new();
    let target = root.0.join("target");
    let entries = serde_json::json!([["file", "root/safe", "new"]]);
    let mut index = IndexedArchive::prepare(
        Cursor::new(archive(&entries)),
        false,
        &fs::File::open(&root.0).unwrap(),
        Some(b"root"),
        &|| false,
    )
    .unwrap();
    let calls = std::cell::Cell::new(0);
    let error = index
        .restore_into(&fs::File::open(&target).unwrap(), &|| {
            calls.set(calls.get() + 1);
            calls.get() >= 4
        })
        .unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::Interrupted);
    assert_eq!(fs::read_dir(&target).unwrap().count(), 0);
    index
        .restore_into(&fs::File::open(&target).unwrap(), &|| false)
        .unwrap();
    assert_eq!(fs::read(target.join("safe")).unwrap(), b"new");
}

#[test]
fn concurrent_leaf_replacement_does_not_follow_a_symlink_to_an_outside_file() {
    let root = Fixture::new();
    let target = root.0.join("target");
    let outside = root.0.join("outside");
    fs::write(&outside, b"sentinel").unwrap();
    fs::write(target.join("victim"), b"old").unwrap();
    let entries = serde_json::json!([
        ["file", "root/marker", "written"],
        ["file", "root/victim", "attacker"]
    ]);
    let mut index = IndexedArchive::prepare(
        Cursor::new(archive(&entries)),
        false,
        &fs::File::open(&root.0).unwrap(),
        Some(b"root"),
        &|| false,
    )
    .unwrap();
    let replaced = std::cell::Cell::new(false);
    let result = index.restore_into(&fs::File::open(&target).unwrap(), &|| {
        if target.join("marker").exists() && !replaced.replace(true) {
            fs::remove_file(target.join("victim")).unwrap();
            symlink(&outside, target.join("victim")).unwrap();
        }
        false
    });
    assert!(replaced.get());
    assert!(result.is_err());
    assert_eq!(fs::read(&outside).unwrap(), b"sentinel");
}

#[test]
fn fractional_pax_timestamps_match_python_utime_floor_semantics() {
    let root = Fixture::new();
    let target = root.0.join("target");
    // Measured through CPython 3.14 os.utime, not derived from the native conversion.
    let cases = [
        ("-0.1", -100_000_000i64),
        ("0.1", 100_000_000),
        ("1.0000000009", 1_000_000_000),
        ("-1.0000000009", -1_000_000_001),
        ("0.0000000015", 1),
        ("-0.0000000015", -2),
        ("1700000000.123456", 1_700_000_000_123_456_001),
    ];
    let mut tar = tar::Builder::new(Vec::new());
    for (index, (literal, _)) in cases.iter().enumerate() {
        let mut header = tar::Header::new_ustar();
        header.set_entry_type(tar::EntryType::Regular);
        header.set_mode(0o640);
        header.set_mtime(0);
        header.set_size(0);
        header.set_cksum();
        tar.append_pax_extensions([("mtime", literal.as_bytes())])
            .unwrap();
        tar.append_data(&mut header, format!("root/{index}"), &b""[..])
            .unwrap();
    }
    let mut index = IndexedArchive::prepare(
        Cursor::new(tar.into_inner().unwrap()),
        false,
        &fs::File::open(&root.0).unwrap(),
        Some(b"root"),
        &|| false,
    )
    .unwrap();
    index
        .restore_into(&fs::File::open(&target).unwrap(), &|| false)
        .unwrap();
    for (index, (literal, expected)) in cases.iter().enumerate() {
        let stat = fs::metadata(target.join(index.to_string())).unwrap();
        assert_eq!(
            stat.mtime() * 1_000_000_000 + stat.mtime_nsec(),
            *expected,
            "{literal}"
        );
    }
}

#[test]
fn archived_symlink_can_replace_an_existing_leaf_link_without_traversing_it() {
    let root = Fixture::new();
    let target = root.0.join("target");
    fs::write(target.join("file"), b"mutated").unwrap();
    symlink("file", target.join("link")).unwrap();
    let entries = serde_json::json!([
        ["file", "root/file", "restored"],
        ["sym", "root/link", "file"]
    ]);
    let mut index = IndexedArchive::prepare(
        Cursor::new(archive(&entries)),
        false,
        &fs::File::open(&root.0).unwrap(),
        Some(b"root"),
        &|| false,
    )
    .unwrap();
    index
        .restore_into(&fs::File::open(&target).unwrap(), &|| false)
        .unwrap();
    assert_eq!(fs::read(target.join("file")).unwrap(), b"restored");
    assert_eq!(
        fs::read_link(target.join("link")).unwrap(),
        std::path::Path::new("file")
    );
}

#[test]
fn preexisting_directory_symlink_attack_rejects_all_writes() {
    let root = Fixture::new();
    let target = root.0.join("target");
    let outside = root.0.join("outside");
    fs::create_dir(&outside).unwrap();
    symlink(&outside, target.join("alias")).unwrap();
    let entries = serde_json::json!([
        ["file", "root/safe", "safe"],
        ["file", "root/alias/escape", "must not escape"]
    ]);
    let mut index = IndexedArchive::prepare(
        Cursor::new(archive(&entries)),
        false,
        &fs::File::open(&root.0).unwrap(),
        Some(b"root"),
        &|| false,
    )
    .unwrap();
    assert!(
        index
            .restore_into(&fs::File::open(&target).unwrap(), &|| false)
            .is_err()
    );
    assert!(!target.join("safe").exists());
    assert_eq!(fs::read_dir(&outside).unwrap().count(), 0);
    assert!(target.join("alias").is_symlink());
}
