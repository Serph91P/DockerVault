use dockervault_core::archive::LayoutScan;

#[test]
fn legacy_single_root_is_detected_without_collecting_members() {
    let mut scan = LayoutScan::new(Some(b""));
    scan.observe(b"old_source", true).unwrap();
    scan.observe(b"old_source/payload", false).unwrap();
    scan.observe(b"old_source/hardlink", false).unwrap();
    let mapping = scan.finish().unwrap();
    assert_eq!(mapping.member(b"old_source", true).unwrap(), b".");
    assert_eq!(
        mapping.member(b"old_source/payload", false).unwrap(),
        b"payload"
    );
    assert_eq!(mapping.hardlink(b"old_source/payload").unwrap(), b"payload");
    assert!(mapping.hardlink(b"other/payload").is_err());
}

#[test]
fn directory_header_trailing_slashes_follow_python_tarinfo_normalization() {
    let mut scan = LayoutScan::new(Some(b""));
    scan.observe(b"root///", true).unwrap();
    scan.observe(b"root/payload", false).unwrap();
    let mapping = scan.finish().unwrap();
    assert_eq!(mapping.member(b"root///", true).unwrap(), b".");
    assert_eq!(mapping.member(b"root/payload", false).unwrap(), b"payload");
}

#[test]
fn container_layout_keeps_roots_even_when_only_one_exists() {
    let mut scan = LayoutScan::new(None);
    scan.observe(b"data/payload", false).unwrap();
    let mapping = scan.finish().unwrap();
    assert_eq!(
        mapping.member(b"data/payload", false).unwrap(),
        b"data/payload"
    );
    assert_eq!(mapping.hardlink(b"data/payload").unwrap(), b"data/payload");
}

#[test]
fn auto_layout_falls_back_to_keep_for_multiple_roots_or_root_named_file() {
    for members in [
        vec![
            (b"data/payload".as_slice(), false),
            (b"logs/payload".as_slice(), false),
        ],
        vec![(b"payload".as_slice(), false)],
    ] {
        let mut scan = LayoutScan::new(Some(b""));
        for &(name, directory) in &members {
            scan.observe(name, directory).unwrap();
        }
        let mapping = scan.finish().unwrap();
        for &(name, directory) in &members {
            assert_eq!(mapping.member(name, directory).unwrap(), name);
        }
    }
}

#[test]
fn recorded_root_mismatch_fails_instead_of_silently_falling_back() {
    let mut scan = LayoutScan::new(Some(b"recorded"));
    scan.observe(b"recorded/payload", false).unwrap();
    scan.observe(b"unexpected/payload", false).unwrap();
    assert!(scan.finish().is_err());
    let mut scan = LayoutScan::new(Some(b"recorded"));
    scan.observe(b"recorded", false).unwrap();
    assert!(scan.finish().is_err());
}

#[test]
fn absolute_or_nul_names_are_rejected_before_root_mapping() {
    for name in [b"/root/payload".as_slice(), b"root/bad\0name".as_slice()] {
        for root in [None, Some(b"".as_slice()), Some(b"root".as_slice())] {
            assert!(LayoutScan::new(root).observe(name, false).is_err());
        }
    }
}

#[test]
fn empty_archive_and_raw_byte_names_match_published_layout_rules() {
    assert!(LayoutScan::new(Some(b"recorded")).finish().is_ok());
    let mut scan = LayoutScan::new(Some(b""));
    scan.observe(b"root/\xff", false).unwrap();
    let mapping = scan.finish().unwrap();
    assert_eq!(mapping.member(b"root/\xff", false).unwrap(), b"\xff");
    assert!(mapping.member(b"outside/payload", false).is_err());
    assert!(mapping.hardlink(b"root").is_err());
}
