//! Ordered disk-backed destination simulation followed by capability-rooted mutation.
use super::{IndexedArchive, Member, MemberKind, index::read_member};
use dockervault_core::archive::member_name;
use nix::{
    fcntl::{AT_FDCWD, AtFlags, OFlag, open, openat},
    sys::{
        stat::{Mode, SFlag, UtimensatFlags, fstatat, futimens, mkdirat, utimensat},
        time::TimeSpec,
    },
    unistd::{UnlinkatFlags, linkat, symlinkat, unlinkat},
};
use rusqlite::{Connection, OptionalExtension, params};
use std::{
    ffi::OsStr,
    fs::File,
    io,
    os::{
        fd::AsRawFd,
        unix::{ffi::OsStrExt, fs::PermissionsExt},
    },
    path::Path,
};
const MAX_PATH: usize = 1024 * 1024;
#[derive(Clone)]
struct Node {
    kind: MemberKind,
    link: Option<Vec<u8>>,
}
struct Planner<'a> {
    db: &'a Connection,
    root: &'a File,
    cancel: &'a dyn Fn() -> bool,
}
struct Step {
    order: i64,
    kind: u8,
    path: Vec<u8>,
    link: Option<Vec<u8>>,
    source: Option<i64>,
    mode: Option<u32>,
    mtime: Option<f64>,
}

impl IndexedArchive {
    /// No mutation occurs until the complete ordered destination simulation succeeds.
    pub fn restore_into(&mut self, root: &File, cancel: &dyn Fn() -> bool) -> io::Result<()> {
        check(cancel)?;
        if !root.metadata()?.is_dir() {
            return Err(invalid("Restore capability is not a directory"));
        }
        self.db.execute_batch("DROP TABLE IF EXISTS restore_nodes; DROP TABLE IF EXISTS restore_steps; CREATE TABLE restore_nodes(path BLOB PRIMARY KEY,kind INTEGER NOT NULL,link BLOB); CREATE TABLE restore_steps(ord INTEGER PRIMARY KEY,kind INTEGER NOT NULL,path BLOB NOT NULL,link BLOB,source INTEGER,mode INTEGER,mtime REAL);").map_err(db_error)?;
        let planner = Planner {
            db: &self.db,
            root,
            cancel,
        };
        planner.put(b".", MemberKind::Directory, None)?;
        {
            let mut statement = self
                .db
                .prepare("SELECT * FROM members ORDER BY seq")
                .map_err(db_error)?;
            let mut rows = statement.query([]).map_err(db_error)?;
            while let Some(row) = rows.next().map_err(db_error)? {
                check(cancel)?;
                let sequence: i64 = row.get("seq").map_err(db_error)?;
                planner.member(sequence, read_member(row).map_err(db_error)?)?;
            }
        }
        check(cancel)?;
        let mut last = 0i64;
        loop {
            let step = self
                .db
                .query_row(
                    "SELECT * FROM restore_steps WHERE ord>?1 ORDER BY ord LIMIT 1",
                    [last],
                    |row| {
                        Ok(Step {
                            order: row.get("ord")?,
                            kind: row.get("kind")?,
                            path: row.get("path")?,
                            link: row.get("link")?,
                            source: row.get("source")?,
                            mode: row.get("mode")?,
                            mtime: row.get("mtime")?,
                        })
                    },
                )
                .optional()
                .map_err(db_error)?;
            let Some(step) = step else { break };
            last = step.order;
            check(cancel)?;
            match step.kind {
                0 => {
                    let (parent, name) = parent_fd(root, &step.path)?;
                    let mut file = regular_output(&parent, &name)?;
                    file.set_len(0)?;
                    self.copy_data(
                        u64::try_from(
                            step.source
                                .ok_or_else(|| invalid("Missing data reference"))?,
                        )
                        .map_err(|_| invalid("Invalid data reference"))?,
                        &mut file,
                        cancel,
                    )?;
                    set_file_attrs(&file, step.mode, step.mtime)?;
                    file.sync_all()?;
                }
                1 => {
                    if step.path != b"." {
                        let (parent, name) = parent_fd(root, &step.path)?;
                        match mkdirat(
                            &parent,
                            OsStr::from_bytes(&name),
                            Mode::from_bits_truncate(0o777),
                        ) {
                            Ok(()) => {}
                            Err(nix::errno::Errno::EEXIST) => {
                                open_dir(root, &step.path)?;
                            }
                            Err(error) => return Err(error.into()),
                        }
                    }
                }
                2 => {
                    let (parent, name) = parent_fd(root, &step.path)?;
                    remove_leaf(&parent, &name)?;
                    symlinkat(
                        OsStr::from_bytes(
                            step.link
                                .as_deref()
                                .ok_or_else(|| invalid("Missing symbolic target"))?,
                        ),
                        &parent,
                        OsStr::from_bytes(&name),
                    )?;
                    if let Some(time) = step.mtime {
                        let time = timestamp(time)?;
                        utimensat(
                            &parent,
                            OsStr::from_bytes(&name),
                            &time,
                            &time,
                            UtimensatFlags::NoFollowSymlink,
                        )?;
                    }
                }
                3 => {
                    let source = step
                        .link
                        .as_deref()
                        .ok_or_else(|| invalid("Missing hardlink reference"))?;
                    let same_path = source == step.path;
                    let (source_parent, source_name) = parent_fd(root, source)?;
                    let source = File::from(openat(
                        &source_parent,
                        OsStr::from_bytes(&source_name),
                        OFlag::O_PATH | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
                        Mode::empty(),
                    )?);
                    if !source.metadata()?.is_file() {
                        return Err(invalid("Hardlink source changed type"));
                    }
                    let descriptor = format!("/proc/self/fd/{}", source.as_raw_fd());
                    // Unlinking the only name would make even the pinned inode un-linkable.
                    if !same_path {
                        let (parent, name) = parent_fd(root, &step.path)?;
                        remove_leaf(&parent, &name)?;
                        linkat(
                            AT_FDCWD,
                            Path::new(&descriptor),
                            &parent,
                            OsStr::from_bytes(&name),
                            AtFlags::AT_SYMLINK_FOLLOW,
                        )?;
                    }
                    let source = File::from(open(
                        Path::new(&descriptor),
                        OFlag::O_RDONLY | OFlag::O_CLOEXEC,
                        Mode::empty(),
                    )?);
                    set_file_attrs(&source, step.mode, step.mtime)?;
                    source.sync_all()?;
                }
                _ => return Err(invalid("Invalid destination operation")),
            }
        }
        // Python applies directory timestamps after descendants; modes/owners are ignored.
        let mut statement=self.db.prepare("SELECT path,mtime FROM restore_steps WHERE kind=1 AND mtime IS NOT NULL GROUP BY path HAVING ord=MAX(ord) ORDER BY length(path) DESC,ord DESC").map_err(db_error)?;
        let mut rows = statement.query([]).map_err(db_error)?;
        while let Some(row) = rows.next().map_err(db_error)? {
            check(cancel)?;
            let path: Vec<u8> = row.get(0).map_err(db_error)?;
            let mtime: f64 = row.get(1).map_err(db_error)?;
            let directory = open_dir(root, &path)?;
            set_file_attrs(&directory, None, Some(mtime))?;
            directory.sync_all()?;
        }
        root.sync_all()?;
        check(cancel)
    }
}
impl Planner<'_> {
    fn put(&self, path: &[u8], kind: MemberKind, link: Option<&[u8]>) -> io::Result<()> {
        self.db.execute("INSERT INTO restore_nodes VALUES(?1,?2,?3) ON CONFLICT(path) DO UPDATE SET kind=excluded.kind,link=excluded.link",params![path,kind as u8,link]).map_err(db_error)?;
        Ok(())
    }
    fn lookup(&self, path: &[u8]) -> io::Result<Option<Node>> {
        check(self.cancel)?;
        let value = self
            .db
            .query_row(
                "SELECT kind,link FROM restore_nodes WHERE path=?1",
                [path],
                |row| Ok((row.get::<_, u8>(0)?, row.get::<_, Option<Vec<u8>>>(1)?)),
            )
            .optional()
            .map_err(db_error)?;
        if let Some((kind, link)) = value {
            return Ok(Some(Node {
                kind: match kind {
                    0 => MemberKind::File,
                    1 => MemberKind::Directory,
                    2 => MemberKind::Symlink,
                    _ => return Err(invalid("Invalid simulated type")),
                },
                link,
            }));
        }
        let (parent, name) = match parent_fd(self.root, path) {
            Ok(value) => value,
            Err(error) if error.raw_os_error() == Some(nix::libc::ENOENT) => return Ok(None),
            Err(error) => return Err(error),
        };
        let stat = match fstatat(
            &parent,
            OsStr::from_bytes(&name),
            AtFlags::AT_SYMLINK_NOFOLLOW,
        ) {
            Ok(stat) => stat,
            Err(nix::errno::Errno::ENOENT) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        let kind = match SFlag::from_bits_truncate(stat.st_mode) & SFlag::S_IFMT {
            SFlag::S_IFDIR => MemberKind::Directory,
            SFlag::S_IFREG => MemberKind::File,
            SFlag::S_IFLNK => MemberKind::Symlink,
            _ => return Err(invalid("Preexisting special destination member")),
        };
        self.put(path, kind, None)?;
        Ok(Some(Node { kind, link: None }))
    }
    fn record(
        &self,
        kind: u8,
        path: &[u8],
        link: Option<&[u8]>,
        source: Option<i64>,
        mode: Option<u32>,
        mtime: Option<f64>,
    ) -> io::Result<()> {
        if let Some(time) = mtime {
            timestamp(time)?;
        }
        self.db.execute("INSERT INTO restore_steps(kind,path,link,source,mode,mtime) VALUES(?1,?2,?3,?4,?5,?6)",params![kind,path,link,source,mode,mtime]).map_err(db_error)?;
        Ok(())
    }
    fn step(&self, path: &[u8]) -> io::Result<Option<Vec<u8>>> {
        if path == b"." {
            return Ok(None);
        }
        let parts: Vec<_> = path.split(|&b| b == b'/').collect();
        let mut prefix = Vec::new();
        for (index, part) in parts.iter().enumerate() {
            let candidate = join(&prefix, part);
            match self.lookup(&candidate)? {
                Some(Node {
                    kind: MemberKind::Symlink,
                    link: Some(target),
                }) => {
                    let mut next = join(&prefix, &target);
                    for suffix in &parts[index + 1..] {
                        next = join(&next, suffix);
                    }
                    if next.len() > MAX_PATH {
                        return Err(invalid("Expanded archive path exceeds bounded limit"));
                    }
                    return Ok(Some(member_name(&next).map_err(invalid)?));
                }
                Some(Node {
                    kind: MemberKind::Symlink,
                    link: None,
                }) => {
                    return Err(invalid(
                        "Preexisting destination symlink cannot be traversed",
                    ));
                }
                Some(Node {
                    kind: MemberKind::File,
                    ..
                }) if index + 1 < parts.len() => {
                    return Err(invalid("Non-directory archive ancestor"));
                }
                None => return Ok(None),
                _ => {}
            }
            prefix = candidate;
        }
        Ok(None)
    }
    // Floyd's walk bounds memory even for a long userland-realpath chain.
    fn resolve(&self, path: &[u8], limit: Option<u32>) -> io::Result<(Vec<u8>, u32)> {
        let mut slow = path.to_vec();
        let mut fast = slow.clone();
        let mut fast_done = false;
        let mut count = 0u32;
        loop {
            check(self.cancel)?;
            let Some(next) = self.step(&slow)? else {
                return Ok((slow, count));
            };
            slow = next;
            count = count
                .checked_add(1)
                .ok_or_else(|| invalid("Archive link walk overflow"))?;
            if limit.is_some_and(|limit| count > limit) {
                return Err(invalid("Archive path exceeds filesystem link limit"));
            }
            if !fast_done {
                for _ in 0..2 {
                    match self.step(&fast)? {
                        Some(next) => fast = next,
                        None => {
                            fast_done = true;
                            break;
                        }
                    }
                }
                if !fast_done && fast == slow {
                    return Err(invalid("Cyclic archive path traversal"));
                }
            }
        }
    }
    fn directories(&self, path: &[u8], create: bool) -> io::Result<()> {
        if path == b"." {
            return Ok(());
        }
        let mut prefix = Vec::new();
        for part in path.split(|&b| b == b'/') {
            prefix = join(&prefix, part);
            match self.lookup(&prefix)? {
                Some(Node {
                    kind: MemberKind::Directory,
                    ..
                }) => {}
                None if create => {
                    self.put(&prefix, MemberKind::Directory, None)?;
                    self.record(1, &prefix, None, None, None, None)?;
                }
                _ => return Err(invalid("Missing or non-directory archive ancestor")),
            }
        }
        Ok(())
    }
    fn parent(&self, path: &[u8]) -> io::Result<(Vec<u8>, Vec<u8>, u32)> {
        let (parent, name) = split(path);
        let (parent, links) = self.resolve(parent, Some(40))?;
        self.directories(&parent, links == 0)?;
        Ok((parent, name.to_vec(), links))
    }
    fn member(&self, sequence: i64, member: Member) -> io::Result<()> {
        let (parent, name, parent_links) = if member.name == b"." {
            (b".".to_vec(), b".".to_vec(), 0)
        } else {
            self.parent(&member.name)?
        };
        let candidate = if member.name == b"." {
            b".".to_vec()
        } else {
            join(&parent, &name)
        };
        match member.kind {
            MemberKind::File | MemberKind::Directory => {
                let (path, links) = self.resolve(&candidate, Some(40 - parent_links))?;
                let (parent, _) = split(&path);
                self.directories(parent, false)?;
                match (member.kind, self.lookup(&path)?) {
                    (
                        MemberKind::File,
                        Some(Node {
                            kind: MemberKind::Directory,
                            ..
                        }),
                    ) => return Err(invalid("File replaces a directory")),
                    (
                        MemberKind::Directory,
                        Some(Node {
                            kind: MemberKind::File,
                            ..
                        }),
                    ) => return Err(invalid("Directory replaces a file")),
                    (MemberKind::Directory, None) if links > 0 => {
                        return Err(invalid("Dangling directory alias"));
                    }
                    _ => {}
                }
                self.put(&path, member.kind, None)?;
                self.record(
                    member.kind as u8,
                    &path,
                    None,
                    Some(sequence),
                    member.data_mode(),
                    Some(member.mtime),
                )?;
            }
            MemberKind::Symlink => {
                if self
                    .lookup(&candidate)?
                    .is_some_and(|node| node.kind == MemberKind::Directory)
                {
                    return Err(invalid("Symbolic link replaces a directory"));
                }
                let target = member
                    .link_name
                    .as_deref()
                    .ok_or_else(|| invalid("Missing symbolic target"))?;
                let reachable = member_name(&join(&parent, target)).map_err(invalid)?;
                self.resolve(&reachable, None)?;
                self.put(&candidate, MemberKind::Symlink, Some(target))?;
                self.record(
                    2,
                    &candidate,
                    Some(target),
                    Some(sequence),
                    None,
                    Some(member.mtime),
                )?;
            }
            MemberKind::Hardlink => {
                if self
                    .lookup(&candidate)?
                    .is_some_and(|node| node.kind == MemberKind::Directory)
                {
                    return Err(invalid("Hardlink replaces a directory"));
                }
                let target = member
                    .link_name
                    .as_deref()
                    .ok_or_else(|| invalid("Missing hardlink target"))?;
                let (target, _) = self.resolve(target, Some(40))?;
                if !self
                    .lookup(&target)?
                    .is_some_and(|node| node.kind == MemberKind::File)
                {
                    return Err(invalid("Missing hardlink source"));
                }
                self.put(&candidate, MemberKind::File, None)?;
                self.record(
                    3,
                    &candidate,
                    Some(&target),
                    Some(sequence),
                    member.data_mode(),
                    Some(member.mtime),
                )?;
            }
        }
        Ok(())
    }
}
fn join(parent: &[u8], child: &[u8]) -> Vec<u8> {
    if parent.is_empty() || parent == b"." {
        child.to_vec()
    } else {
        let mut out = parent.to_vec();
        out.push(b'/');
        out.extend_from_slice(child);
        out
    }
}
fn split(path: &[u8]) -> (&[u8], &[u8]) {
    match path.iter().rposition(|&b| b == b'/') {
        Some(i) => (&path[..i], &path[i + 1..]),
        None => (b".", path),
    }
}
fn open_dir(root: &File, path: &[u8]) -> io::Result<File> {
    let mut current = root.try_clone()?;
    if path == b"." {
        return Ok(current);
    }
    for part in path.split(|&b| b == b'/') {
        current = File::from(openat(
            &current,
            OsStr::from_bytes(part),
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::empty(),
        )?);
    }
    Ok(current)
}
fn parent_fd(root: &File, path: &[u8]) -> io::Result<(File, Vec<u8>)> {
    let (parent, name) = split(path);
    Ok((open_dir(root, parent)?, name.to_vec()))
}
fn remove_leaf(parent: &File, name: &[u8]) -> io::Result<()> {
    match unlinkat(parent, OsStr::from_bytes(name), UnlinkatFlags::NoRemoveDir) {
        Ok(()) | Err(nix::errno::Errno::ENOENT) => Ok(()),
        Err(error) => Err(error.into()),
    }
}
fn regular_output(parent: &File, name: &[u8]) -> io::Result<File> {
    match openat(
        parent,
        OsStr::from_bytes(name),
        OFlag::O_PATH | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    ) {
        Ok(fd) => {
            let handle = File::from(fd);
            if !handle.metadata()?.is_file() {
                return Err(invalid("Destination file changed type"));
            }
            let descriptor = format!("/proc/self/fd/{}", handle.as_raw_fd());
            Ok(File::from(open(
                Path::new(&descriptor),
                OFlag::O_WRONLY | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
                Mode::empty(),
            )?))
        }
        Err(nix::errno::Errno::ENOENT) => Ok(File::from(openat(
            parent,
            OsStr::from_bytes(name),
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::from_bits_truncate(0o600),
        )?)),
        Err(error) => Err(error.into()),
    }
}
fn timestamp(value: f64) -> io::Result<TimeSpec> {
    let seconds = value.floor();
    if !value.is_finite() || seconds < i64::MIN as f64 || seconds >= i64::MAX as f64 {
        return Err(invalid("Invalid restore timestamp"));
    }
    let mut seconds = seconds as i64;
    let mut nanos = ((value - seconds as f64) * 1e9).floor() as i64;
    if nanos == 1_000_000_000 {
        seconds = seconds
            .checked_add(1)
            .ok_or_else(|| invalid("Timestamp overflow"))?;
        nanos = 0;
    }
    Ok(TimeSpec::new(seconds, nanos))
}
fn set_file_attrs(file: &File, mode: Option<u32>, mtime: Option<f64>) -> io::Result<()> {
    if let Some(mode) = mode {
        file.set_permissions(std::fs::Permissions::from_mode(mode))?;
    }
    if let Some(time) = mtime {
        let time = timestamp(time)?;
        futimens(file, &time, &time)?;
    }
    Ok(())
}
fn check(cancel: &dyn Fn() -> bool) -> io::Result<()> {
    if cancel() {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Restore cancelled",
        ))
    } else {
        Ok(())
    }
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn db_error(error: rusqlite::Error) -> io::Error {
    io::Error::other(error)
}
