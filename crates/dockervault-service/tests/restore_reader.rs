use dockervault_service::restore::ArchiveReader;
use std::io::Cursor;

fn archive(extension: Option<(&str, &[u8])>, kind: u8) -> Vec<u8> {
    let mut tar = tar::Builder::new(Vec::new());
    if let Some(extension) = extension {
        tar.append_pax_extensions([extension]).unwrap();
    }
    let mut h = tar::Header::new_ustar();
    h.set_entry_type(tar::EntryType::new(kind));
    h.set_mode(0o6755);
    h.set_mtime(1234567890);
    h.set_size(if kind == b'0' { 4 } else { 0 });
    h.set_cksum();
    tar.append_data(
        &mut h,
        "root/payload",
        if kind == b'0' {
            b"data".as_slice()
        } else {
            b""
        },
    )
    .unwrap();
    tar.into_inner().unwrap()
}

#[test]
fn bounded_reader_keeps_effective_pax_path_time_and_data_offset() {
    let mut bytes = tar::Builder::new(Vec::new());
    bytes
        .append_pax_extensions([
            ("path", b"root/overridden".as_slice()),
            ("mtime", b"1234567890.25".as_slice()),
        ])
        .unwrap();
    let mut h = tar::Header::new_ustar();
    h.set_size(4);
    h.set_mode(0o644);
    h.set_cksum();
    bytes
        .append_data(&mut h, "base", b"data".as_slice())
        .unwrap();
    let bytes = bytes.into_inner().unwrap();
    let members = ArchiveReader::new(Cursor::new(&bytes))
        .unwrap()
        .collect::<std::io::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].name, b"root/overridden");
    assert_eq!(members[0].mtime, 1234567890.25);
    let offset = usize::try_from(members[0].data_offset).unwrap();
    assert_eq!(&bytes[offset..offset + 4], b"data");
}

#[test]
fn effective_data_modes_ignore_directory_and_symlink_modes() {
    for (kind, expected) in [(b'0', Some(0o755)), (b'5', None), (b'2', None)] {
        let member = ArchiveReader::new(Cursor::new(archive(None, kind)))
            .unwrap()
            .next()
            .unwrap()
            .unwrap();
        assert_eq!(member.data_mode(), expected);
    }
}

#[test]
fn global_pax_is_applied_instead_of_exposing_a_fake_member() {
    let bytes = include_bytes!("fixtures/global-pax.TEST_ONLY.tar");
    let members = ArchiveReader::new(Cursor::new(bytes))
        .unwrap()
        .collect::<std::io::Result<Vec<_>>>()
        .unwrap();
    assert_eq!(members.len(), 1);
    assert_eq!(members[0].name, b"global/payload");
    assert_eq!(members[0].size, 4);
}

#[test]
fn pax_override_is_not_hidden_by_a_safe_ustar_name() {
    let bytes = archive(Some(("path", b"../escape")), b'0');
    let member = ArchiveReader::new(Cursor::new(bytes))
        .unwrap()
        .next()
        .unwrap()
        .unwrap();
    assert_eq!(member.name, b"../escape");
}

#[test]
fn special_file_and_partial_payload_are_rejected() {
    assert!(
        ArchiveReader::new(Cursor::new(archive(None, b'6')))
            .unwrap()
            .next()
            .unwrap()
            .is_err()
    );
    let mut bytes = archive(None, b'0');
    bytes.truncate(514);
    assert!(
        ArchiveReader::new(Cursor::new(bytes))
            .unwrap()
            .next()
            .unwrap()
            .is_err()
    );
}

#[test]
fn oversized_extension_is_rejected_before_reading_or_allocating_its_payload() {
    use std::io::{Read, Seek, SeekFrom};
    struct Guarded(Cursor<Vec<u8>>, u64);
    impl Read for Guarded {
        fn read(&mut self, buffer: &mut [u8]) -> std::io::Result<usize> {
            assert!(
                self.0.position() < 512,
                "oversized metadata payload must not be read"
            );
            self.0.read(buffer)
        }
    }
    impl Seek for Guarded {
        fn seek(&mut self, position: SeekFrom) -> std::io::Result<u64> {
            match position {
                SeekFrom::End(offset) => self.0.seek(SeekFrom::Start(
                    u64::try_from(i128::from(self.1) + i128::from(offset)).unwrap(),
                )),
                other => self.0.seek(other),
            }
        }
    }
    let mut header = tar::Header::new_ustar();
    header.set_entry_type(tar::EntryType::XHeader);
    header.set_size(1 << 21);
    header.set_mode(0o600);
    header.set_cksum();
    let source = Guarded(
        Cursor::new(header.as_bytes().to_vec()),
        512 + header.entry_size().unwrap(),
    );
    let error = ArchiveReader::new(source)
        .unwrap()
        .next()
        .unwrap()
        .err()
        .unwrap();
    assert!(error.to_string().contains("bounded limit"));
}
