use super::{ArchiveReader, Member, MemberKind};
use dockervault_core::archive::LayoutScan;
use rusqlite::{Connection, params};
use std::{
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
};

/// Disk-backed source metadata, NOT destination/link-graph validation or permission to extract.
pub struct IndexedArchive {
    db: Connection,
    snapshot: File,
    count: u64,
}
impl IndexedArchive {
    pub fn prepare(
        source: impl Read,
        compressed: bool,
        spool_directory: &File,
        strip_root: Option<&[u8]>,
        cancel: &dyn Fn() -> bool,
    ) -> io::Result<Self> {
        check(cancel)?;
        let mut snapshot = File::from(nix::fcntl::openat(
            spool_directory,
            ".",
            nix::fcntl::OFlag::O_RDWR | nix::fcntl::OFlag::O_TMPFILE | nix::fcntl::OFlag::O_CLOEXEC,
            nix::sys::stat::Mode::from_bits_truncate(0o600),
        )?);
        let mut source: Box<dyn Read + '_> = if compressed {
            Box::new(flate2::read::MultiGzDecoder::new(source))
        } else {
            Box::new(source)
        };
        let mut buffer = [0u8; 65536];
        loop {
            check(cancel)?;
            let count = source.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            snapshot.write_all(&buffer[..count])?;
        }
        snapshot.rewind()?;
        // SQLite's empty filename is a private on-disk temporary DB, deleted on close.
        let mut db = Connection::open("").map_err(db_error)?;
        db.execute_batch("PRAGMA cache_size=-1024; PRAGMA mmap_size=0; PRAGMA temp_store=FILE; CREATE TABLE members(seq INTEGER PRIMARY KEY,name BLOB NOT NULL,link BLOB,kind INTEGER NOT NULL,size INTEGER NOT NULL,data_offset INTEGER NOT NULL,mode INTEGER NOT NULL,mtime REAL NOT NULL);").map_err(db_error)?;
        let transaction = db.transaction().map_err(db_error)?;
        let mut scan = LayoutScan::new(strip_root);
        let mut count = 0u64;
        for member in ArchiveReader::new(&mut snapshot)? {
            check(cancel)?;
            let member = member?;
            scan.observe(&member.name, member.kind == MemberKind::Directory)
                .map_err(invalid)?;
            count += 1;
            transaction
                .execute(
                    "INSERT INTO members VALUES(?1,?2,?3,?4,?5,?6,?7,?8)",
                    params![
                        signed(count)?,
                        member.name,
                        member.link_name,
                        member.kind as u8,
                        signed(member.size)?,
                        signed(member.data_offset)?,
                        member.mode,
                        member.mtime
                    ],
                )
                .map_err(db_error)?;
        }
        let mapping = scan.finish().map_err(invalid)?;
        {
            let mut statement = transaction
                .prepare("SELECT * FROM members ORDER BY seq")
                .map_err(db_error)?;
            let mut rows = statement.query([]).map_err(db_error)?;
            while let Some(row) = rows.next().map_err(db_error)? {
                check(cancel)?;
                let mut member = read_member(row).map_err(db_error)?;
                member.name = mapping
                    .member(&member.name, member.kind == MemberKind::Directory)
                    .map_err(invalid)?;
                if member.kind == MemberKind::Hardlink {
                    member.link_name = Some(
                        mapping
                            .hardlink(
                                member
                                    .link_name
                                    .as_deref()
                                    .ok_or_else(|| invalid("Missing hardlink target"))?,
                            )
                            .map_err(invalid)?,
                    );
                }
                let seq: i64 = row.get("seq").map_err(db_error)?;
                transaction
                    .execute(
                        "UPDATE members SET name=?1,link=?2 WHERE seq=?3",
                        params![member.name, member.link_name, seq],
                    )
                    .map_err(db_error)?;
            }
        }
        check(cancel)?;
        transaction.commit().map_err(db_error)?;
        Ok(Self {
            db,
            snapshot,
            count,
        })
    }
    pub fn member_count(&self) -> u64 {
        self.count
    }
    pub fn visit(&self, mut visitor: impl FnMut(Member) -> io::Result<()>) -> io::Result<()> {
        let mut statement = self
            .db
            .prepare("SELECT * FROM members ORDER BY seq")
            .map_err(db_error)?;
        let mut rows = statement.query([]).map_err(db_error)?;
        while let Some(row) = rows.next().map_err(db_error)? {
            visitor(read_member(row).map_err(db_error)?)?;
        }
        Ok(())
    }
    /// Copies into caller-owned staging only; live destination mutation requires separate validation.
    pub fn copy_data(
        &mut self,
        sequence: u64,
        output: &mut impl Write,
        cancel: &dyn Fn() -> bool,
    ) -> io::Result<()> {
        let member = self
            .db
            .query_row(
                "SELECT * FROM members WHERE seq=?1",
                [signed(sequence)?],
                read_member,
            )
            .map_err(db_error)?;
        if member.kind != MemberKind::File {
            return Err(invalid("Member is not a regular file"));
        }
        self.snapshot.seek(SeekFrom::Start(member.data_offset))?;
        let mut remaining = member.size;
        let mut buffer = [0u8; 65536];
        while remaining > 0 {
            check(cancel)?;
            let size = usize::try_from(remaining.min(buffer.len() as u64)).unwrap();
            let count = self.snapshot.read(&mut buffer[..size])?;
            if count == 0 {
                return Err(invalid("Truncated private archive snapshot"));
            }
            output.write_all(&buffer[..count])?;
            remaining -= count as u64;
        }
        check(cancel)
    }
}
fn read_member(row: &rusqlite::Row<'_>) -> rusqlite::Result<Member> {
    let value: u8 = row.get("kind")?;
    let kind = match value {
        0 => MemberKind::File,
        1 => MemberKind::Directory,
        2 => MemberKind::Symlink,
        3 => MemberKind::Hardlink,
        _ => {
            return Err(rusqlite::Error::IntegralValueOutOfRange(
                3,
                i64::from(value),
            ));
        }
    };
    Ok(Member {
        name: row.get("name")?,
        link_name: row.get("link")?,
        kind,
        size: unsigned(row, 4)?,
        data_offset: unsigned(row, 5)?,
        mode: row.get("mode")?,
        mtime: row.get("mtime")?,
    })
}
fn signed(value: u64) -> io::Result<i64> {
    i64::try_from(value).map_err(|_| invalid("Archive field exceeds SQLite integer range"))
}
fn unsigned(row: &rusqlite::Row<'_>, index: usize) -> rusqlite::Result<u64> {
    let value: i64 = row.get(index)?;
    u64::try_from(value).map_err(|_| rusqlite::Error::IntegralValueOutOfRange(index, value))
}
fn check(cancel: &dyn Fn() -> bool) -> io::Result<()> {
    if cancel() {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Archive indexing cancelled",
        ))
    } else {
        Ok(())
    }
}
fn db_error(error: rusqlite::Error) -> io::Error {
    io::Error::other(error)
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
