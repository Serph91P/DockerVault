use dockervault_service::restore::IndexedArchive;
use std::{
    fs,
    io::Cursor,
    sync::atomic::{AtomicU64, Ordering},
};
static NEXT: AtomicU64 = AtomicU64::new(0);
struct Fixture(std::path::PathBuf);
impl Fixture {
    fn new() -> Self {
        let p = std::env::temp_dir().join(format!(
            "dv-restore-index-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&p).unwrap();
        Self(p)
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        fs::remove_dir_all(&self.0).unwrap();
    }
}
fn archive(special: bool) -> Vec<u8> {
    let mut tar = tar::Builder::new(Vec::new());
    let mut h = tar::Header::new_ustar();
    h.set_size(4);
    h.set_mode(0o640);
    h.set_cksum();
    tar.append_data(&mut h, "root/payload", b"data".as_slice())
        .unwrap();
    if special {
        let mut h = tar::Header::new_ustar();
        h.set_entry_type(tar::EntryType::Fifo);
        h.set_size(0);
        h.set_mode(0o600);
        h.set_cksum();
        tar.append_data(&mut h, "root/fifo", std::io::empty())
            .unwrap();
    }
    tar.into_inner().unwrap()
}
#[test]
fn complete_stream_is_spooled_and_layout_indexed_without_named_staging() {
    for compressed in [false, true] {
        let root = Fixture::new();
        let bytes = archive(false);
        let input = if compressed {
            use std::io::Write;
            let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
            gzip.write_all(&bytes).unwrap();
            gzip.finish().unwrap()
        } else {
            bytes
        };
        let mut index = IndexedArchive::prepare(
            Cursor::new(input),
            compressed,
            &fs::File::open(&root.0).unwrap(),
            Some(b"root"),
            &|| false,
        )
        .unwrap();
        assert_eq!(index.member_count(), 1);
        let mut names = Vec::new();
        index
            .visit(|member| {
                names.push(member.name);
                Ok(())
            })
            .unwrap();
        assert_eq!(names, vec![b"payload".to_vec()]);
        let mut data = Vec::new();
        index.copy_data(1, &mut data, &|| false).unwrap();
        assert_eq!(data, b"data");
        assert_eq!(fs::read_dir(&root.0).unwrap().count(), 0);
        drop(index);
        assert_eq!(fs::read_dir(&root.0).unwrap().count(), 0);
    }
}
#[test]
fn late_fifo_and_recorded_layout_mismatch_never_return_an_index() {
    let root = Fixture::new();
    let fd = fs::File::open(&root.0).unwrap();
    assert!(
        IndexedArchive::prepare(
            Cursor::new(archive(true)),
            false,
            &fd,
            Some(b"root"),
            &|| false
        )
        .is_err()
    );
    assert!(
        IndexedArchive::prepare(
            Cursor::new(archive(false)),
            false,
            &fd,
            Some(b"wrong"),
            &|| false
        )
        .is_err()
    );
    assert_eq!(fs::read_dir(&root.0).unwrap().count(), 0);
}
#[test]
fn cancellation_or_bad_gzip_footer_does_not_return_a_partial_index() {
    let root = Fixture::new();
    let fd = fs::File::open(&root.0).unwrap();
    assert!(
        IndexedArchive::prepare(Cursor::new(archive(false)), false, &fd, None, &|| true).is_err()
    );
    use std::io::Write;
    let mut gzip = flate2::write::GzEncoder::new(Vec::new(), flate2::Compression::new(6));
    gzip.write_all(&archive(false)).unwrap();
    let mut data = gzip.finish().unwrap();
    let i = data.len() - 8;
    data[i] ^= 1;
    assert!(IndexedArchive::prepare(Cursor::new(data), true, &fd, None, &|| false).is_err());
    assert_eq!(fs::read_dir(&root.0).unwrap().count(), 0);
}
