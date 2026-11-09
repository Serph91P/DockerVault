use super::Request;
use dockervault_core::filters::Filters;
use nix::{
    dir::Dir,
    fcntl::{AtFlags, OFlag, open, openat, readlinkat},
    sys::stat::{Mode, SFlag, fstatat, mkdirat},
    unistd::{UnlinkatFlags, linkat, unlinkat},
};
use std::{
    fs::File,
    io::{self, Read, Write},
    os::unix::{
        ffi::{OsStrExt, OsStringExt},
        fs::MetadataExt,
    },
    path::{Component, Path, PathBuf},
};

#[derive(Default)]
pub struct Report {
    pub skipped_files: Vec<PathBuf>,
}

/// The binary supplies its fixed /backups directory capability; never a caller-selected output root.
pub fn create_archive(
    request: &Request,
    backups: &File,
    cancel: &dyn Fn() -> bool,
) -> io::Result<Report> {
    check(cancel)?;
    let relative = request
        .dest
        .strip_prefix("/backups")
        .map_err(|_| invalid("Invalid destination"))?;
    let filename = relative
        .file_name()
        .ok_or_else(|| invalid("Missing destination filename"))?;
    let parent = descend(backups, relative.parent().unwrap_or(Path::new("")))?;
    if fstatat(&parent, filename, AtFlags::AT_SYMLINK_NOFOLLOW).is_ok() {
        return Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Archive already exists",
        ));
    }
    // Freeze all approved source directory capabilities before creating any output.
    let mut sources = Vec::new();
    for (name, path) in &request.sources {
        let real = std::fs::canonicalize(path)?;
        let mut approved = real.starts_with("/var/lib/docker/volumes")
            || real.starts_with("/var/lib/docker/overlay2");
        for prefix in &request.allowed_sources {
            if real.starts_with(std::fs::canonicalize(prefix)?) {
                approved = true;
            }
        }
        if !approved {
            return Err(invalid("Source is outside allowed prefixes"));
        }
        let root = File::from(open(Path::new("/"), directory_flags(), Mode::empty())?);
        let source = descend(&root, real.strip_prefix("/").unwrap())?;
        let filters = request.filters_for(name).map_err(invalid)?;
        sources.push((name, source, path, filters));
    }
    let stage = Stage::new(&parent)?;
    let mut output = stage.file("archive.partial", true)?;
    let mut report = Report::default();
    if request.compress {
        let gzip = flate2::write::GzEncoder::new(&mut output, flate2::Compression::new(6));
        let mut archive = tar::Builder::new(gzip);
        archive.follow_symlinks(false);
        archive.sparse(false);
        for (name, source, original, filters) in &sources {
            walk(
                &mut archive,
                source,
                original,
                Path::new(name),
                Path::new(""),
                &WalkOptions {
                    stage: &stage,
                    filters,
                    cancel,
                },
                &mut report,
            )?;
        }
        archive.into_inner()?.finish()?;
    } else {
        let mut archive = tar::Builder::new(&mut output);
        archive.follow_symlinks(false);
        archive.sparse(false);
        for (name, source, original, filters) in &sources {
            walk(
                &mut archive,
                source,
                original,
                Path::new(name),
                Path::new(""),
                &WalkOptions {
                    stage: &stage,
                    filters,
                    cancel,
                },
                &mut report,
            )?;
        }
        archive.into_inner()?.flush()?;
    }
    if nix::unistd::geteuid().is_root()
        && let (Ok(uid), Ok(gid)) = (std::env::var("SUDO_UID"), std::env::var("SUDO_GID"))
    {
        let uid = uid
            .parse::<u32>()
            .map_err(|_| invalid("Invalid caller uid"))?;
        let gid = gid
            .parse::<u32>()
            .map_err(|_| invalid("Invalid caller gid"))?;
        nix::unistd::fchown(
            &output,
            Some(nix::unistd::Uid::from_raw(uid)),
            Some(nix::unistd::Gid::from_raw(gid)),
        )?;
    }
    output.sync_all()?;
    check(cancel)?;
    // Atomic no-replace publication; existing recovery points can never be overwritten.
    linkat(
        &stage.directory,
        "archive.partial",
        &parent,
        filename,
        AtFlags::empty(),
    )?;
    parent.sync_all()?;
    Ok(report)
}

fn directory_flags() -> OFlag {
    OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC
}
fn descend(root: &File, path: &Path) -> io::Result<File> {
    let mut directory = root.try_clone()?;
    for component in path.components() {
        match component {
            Component::Normal(name) => {
                directory = File::from(openat(&directory, name, directory_flags(), Mode::empty())?)
            }
            Component::CurDir => {}
            _ => return Err(invalid("Invalid directory component")),
        }
    }
    Ok(directory)
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidInput, message)
}
fn check(cancel: &dyn Fn() -> bool) -> io::Result<()> {
    if cancel() {
        Err(io::Error::other("Archive creation cancelled"))
    } else {
        Ok(())
    }
}

struct Stage {
    parent: File,
    directory: File,
    name: String,
}
impl Stage {
    fn new(parent: &File) -> io::Result<Self> {
        let mut random = [0u8; 16];
        getrandom::fill(&mut random)
            .map_err(|_| io::Error::other("Private staging unavailable"))?;
        let name = format!(
            ".dockervault-archive-{}",
            random
                .iter()
                .map(|b| format!("{b:02x}"))
                .collect::<String>()
        );
        mkdirat(parent, name.as_str(), Mode::S_IRWXU)?;
        let directory = File::from(openat(
            parent,
            name.as_str(),
            directory_flags(),
            Mode::empty(),
        )?);
        Ok(Self {
            parent: parent.try_clone()?,
            directory,
            name,
        })
    }
    fn file(&self, name: &str, create: bool) -> io::Result<File> {
        let flags = if create {
            OFlag::O_WRONLY | OFlag::O_CREAT | OFlag::O_EXCL
        } else {
            OFlag::O_RDONLY
        };
        Ok(File::from(openat(
            &self.directory,
            name,
            flags | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
            Mode::S_IRUSR | Mode::S_IWUSR,
        )?))
    }
    fn known_link(&self, metadata: &std::fs::Metadata) -> io::Result<Option<PathBuf>> {
        if metadata.nlink() < 2 {
            return Ok(None);
        }
        let name = format!("link-{:x}-{:x}", metadata.dev(), metadata.ino());
        match self.file(&name, false) {
            Ok(mut file) => {
                let mut data = Vec::new();
                file.read_to_end(&mut data)?;
                Ok(Some(std::ffi::OsString::from_vec(data).into()))
            }
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(error),
        }
    }
    fn remember_link(&self, metadata: &std::fs::Metadata, path: &Path) -> io::Result<()> {
        if metadata.nlink() >= 2 {
            self.file(
                &format!("link-{:x}-{:x}", metadata.dev(), metadata.ino()),
                true,
            )?
            .write_all(path.as_os_str().as_bytes())?;
        }
        Ok(())
    }
}
impl Drop for Stage {
    fn drop(&mut self) {
        if let Ok(fd) = openat(&self.directory, ".", directory_flags(), Mode::empty())
            && let Ok(mut entries) = Dir::from_fd(fd)
        {
            for entry in entries.iter().flatten() {
                let name = entry.file_name();
                if name.to_bytes() != b"." && name.to_bytes() != b".." {
                    let _ = unlinkat(&self.directory, name, UnlinkatFlags::NoRemoveDir);
                }
            }
        }
        let _ = unlinkat(&self.parent, self.name.as_str(), UnlinkatFlags::RemoveDir);
    }
}

struct CancelReader<'a> {
    file: &'a mut File,
    cancel: &'a dyn Fn() -> bool,
}
impl Read for CancelReader<'_> {
    fn read(&mut self, output: &mut [u8]) -> io::Result<usize> {
        check(self.cancel)?;
        let limit = output.len().min(1024 * 1024);
        self.file.read(&mut output[..limit])
    }
}

struct WalkOptions<'a> {
    stage: &'a Stage,
    filters: &'a Filters,
    cancel: &'a dyn Fn() -> bool,
}
fn walk<W: Write>(
    archive: &mut tar::Builder<W>,
    directory: &File,
    original: &Path,
    archive_path: &Path,
    relative: &Path,
    options: &WalkOptions<'_>,
    report: &mut Report,
) -> io::Result<()> {
    let stage = options.stage;
    let filters = options.filters;
    let cancel = options.cancel;
    check(cancel)?;
    let metadata = directory.metadata()?;
    append(
        archive,
        archive_path,
        &metadata,
        tar::EntryType::Directory,
        None,
        io::empty(),
    )?;
    let mut entries = Dir::from_fd(openat(directory, ".", directory_flags(), Mode::empty())?)?;
    for result in entries.iter() {
        check(cancel)?;
        let entry = result?;
        let name = entry.file_name();
        if name.to_bytes() == b"." || name.to_bytes() == b".." {
            continue;
        }
        let component = std::ffi::OsStr::from_bytes(name.to_bytes());
        let relative = relative.join(component);
        let archive_path = archive_path.join(component);
        let original = original.join(component);
        let stat = match fstatat(directory, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
            Ok(stat) => stat,
            Err(_) => {
                report.skipped_files.push(original);
                continue;
            }
        };
        let kind = stat.st_mode & SFlag::S_IFMT.bits();
        let filter_path = relative.as_os_str().as_bytes();
        if kind == SFlag::S_IFDIR.bits() {
            if !filters.traverses_bytes(filter_path) {
                continue;
            }
            match openat(directory, name, directory_flags(), Mode::empty()) {
                Ok(fd) => walk(
                    archive,
                    &File::from(fd),
                    &original,
                    &archive_path,
                    &relative,
                    options,
                    report,
                )?,
                Err(_) => report.skipped_files.push(original),
            }
        } else if filters.includes_bytes(filter_path) {
            if kind == SFlag::S_IFLNK.bits() {
                let target = readlinkat(directory, name)?;
                // Symlink metadata cannot be obtained through a dereferenced path.
                let mut header = tar::Header::new_ustar();
                header.set_mode(stat.st_mode & 0o7777);
                header.set_uid(stat.st_uid.into());
                header.set_gid(stat.st_gid.into());
                header.set_mtime(stat.st_mtime.max(0) as u64);
                header.set_size(0);
                header.set_entry_type(tar::EntryType::Symlink);
                let time = pax_time(stat.st_mtime, stat.st_mtime_nsec);
                append_header(
                    archive,
                    &archive_path,
                    header,
                    Some(Path::new(std::ffi::OsStr::from_bytes(target.as_bytes()))),
                    time.as_deref(),
                    io::empty(),
                )?;
            } else if kind == SFlag::S_IFREG.bits() {
                let mut file = match openat(
                    directory,
                    name,
                    OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
                    Mode::empty(),
                ) {
                    Ok(fd) => File::from(fd),
                    Err(_) => {
                        report.skipped_files.push(original);
                        continue;
                    }
                };
                let metadata = file.metadata()?;
                if !metadata.is_file()
                    || metadata.dev() != stat.st_dev
                    || metadata.ino() != stat.st_ino
                {
                    report.skipped_files.push(original);
                    continue;
                }
                if let Some(target) = stage.known_link(&metadata)? {
                    append(
                        archive,
                        &archive_path,
                        &metadata,
                        tar::EntryType::Link,
                        Some(&target),
                        io::empty(),
                    )?;
                } else {
                    append(
                        archive,
                        &archive_path,
                        &metadata,
                        tar::EntryType::Regular,
                        None,
                        CancelReader {
                            file: &mut file,
                            cancel,
                        },
                    )?;
                    stage.remember_link(&metadata, &archive_path)?;
                }
            } else {
                return Err(invalid("Unsupported archive source file type"));
            }
        }
    }
    Ok(())
}

fn append<W: Write, R: Read>(
    archive: &mut tar::Builder<W>,
    path: &Path,
    metadata: &std::fs::Metadata,
    kind: tar::EntryType,
    link: Option<&Path>,
    data: R,
) -> io::Result<()> {
    let mut header = tar::Header::new_ustar();
    header.set_mode(metadata.mode() & 0o7777);
    header.set_uid(metadata.uid().into());
    header.set_gid(metadata.gid().into());
    header.set_mtime(metadata.mtime().max(0) as u64);
    header.set_entry_type(kind);
    header.set_size(if kind.is_file() { metadata.len() } else { 0 });
    let time = pax_time(metadata.mtime(), metadata.mtime_nsec());
    append_header(archive, path, header, link, time.as_deref(), data)
}

fn pax_time(seconds: i64, fraction: i64) -> Option<String> {
    if seconds < 0 || fraction != 0 {
        let nanos = i128::from(seconds) * 1_000_000_000 + i128::from(fraction);
        let magnitude = nanos.abs();
        Some(format!(
            "{}{}.{:09}",
            if nanos < 0 { "-" } else { "" },
            magnitude / 1_000_000_000,
            magnitude % 1_000_000_000
        ))
    } else {
        None
    }
}
fn append_header<W: Write, R: Read>(
    archive: &mut tar::Builder<W>,
    path: &Path,
    mut header: tar::Header,
    link: Option<&Path>,
    mtime: Option<&str>,
    data: R,
) -> io::Result<()> {
    let path_bytes = path.as_os_str().as_bytes();
    let mut extensions = Vec::new();
    if std::str::from_utf8(path_bytes).is_err()
        || link.is_some_and(|p| std::str::from_utf8(p.as_os_str().as_bytes()).is_err())
    {
        extensions.push(("hdrcharset", b"BINARY".as_slice()));
    }
    if path_bytes.len() > 100 || !path_bytes.is_ascii() {
        extensions.push(("path", path_bytes));
        header.set_path("entry")?;
    } else {
        header.set_path(path)?;
    }
    if let Some(link) = link {
        let bytes = link.as_os_str().as_bytes();
        if bytes.len() > 100 || !bytes.is_ascii() {
            extensions.push(("linkpath", bytes));
            header.set_link_name("link")?;
        } else {
            header.set_link_name(link)?;
        }
    }
    if let Some(time) = mtime {
        extensions.push(("mtime", time.as_bytes()));
    }
    if !extensions.is_empty() {
        archive.append_pax_extensions(extensions)?;
    }
    header.set_cksum();
    archive.append(&header, data)
}
