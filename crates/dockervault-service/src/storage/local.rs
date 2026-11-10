//! FD-rooted local/network transfer; workers retain capacity through fsync and readback.
use super::{FileIdentity, VerifiedSource, check, invalid};
use crate::ownership::{Owner, Work, WorkError};
use nix::{
    fcntl::{OFlag, openat, renameat},
    sys::{
        stat::{Mode, futimens, mkdirat},
        time::TimeSpec,
    },
    unistd::{UnlinkatFlags, unlinkat},
};
use sha2::{Digest, Sha256};
use std::{
    ffi::{OsStr, OsString},
    fs::File,
    io::{self, Read, Write},
    os::unix::fs::{MetadataExt, PermissionsExt},
    path::{Component, Path, PathBuf},
    sync::Arc,
};

#[derive(Debug)]
pub enum TransferError {
    Io(io::Error),
    PublicationUncertain(io::Error),
}
impl std::fmt::Display for TransferError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Io(_) => "Local transfer failed",
            Self::PublicationUncertain(_) => "Local transfer publication durability is uncertain",
        })
    }
}
impl std::error::Error for TransferError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        Some(match self {
            Self::Io(e) | Self::PublicationUncertain(e) => e,
        })
    }
}
impl From<io::Error> for TransferError {
    fn from(error: io::Error) -> Self {
        Self::Io(error)
    }
}
#[derive(Debug)]
pub struct Downloaded {
    length: u64,
    digest: [u8; 32],
}
impl Downloaded {
    pub fn len(&self) -> u64 {
        self.length
    }
    pub fn is_empty(&self) -> bool {
        self.length == 0
    }
    pub fn sha256_hex(&self) -> String {
        self.digest.iter().map(|b| format!("{b:02x}")).collect()
    }
}
#[derive(Clone)]
pub struct LocalStorage {
    root: Arc<File>,
    workers: Owner,
}
impl LocalStorage {
    pub fn new(root: File, workers: Owner) -> io::Result<Self> {
        if !root.metadata()?.is_dir() {
            return Err(invalid("Storage root is not a directory"));
        }
        Ok(Self {
            root: Arc::new(root),
            workers,
        })
    }
    pub fn upload(
        &self,
        source: Arc<VerifiedSource>,
        remote: PathBuf,
    ) -> Result<Work<Result<(), TransferError>>, WorkError> {
        let root = self.root.clone();
        self.workers.submit_blocking(move |cancel| {
            let cancelled = || cancel.is_cancelled();
            check(&cancelled)?;
            source.check_unchanged()?;
            let (parent, name) = parent(&root, &remote, true)?;
            let source_file = source.reopen()?;
            if let Ok(destination) = read_leaf(&parent, &name) {
                let a = source_file.metadata()?;
                let b = destination.metadata()?;
                if a.dev() == b.dev() && a.ino() == b.ino() {
                    return Err(invalid("Source and destination are the same file").into());
                }
            }
            let mut staging = Staging::new(parent)?;
            copy_complete(source_file, &mut staging.file, &cancelled)?;
            source.check_unchanged()?;
            staging.publish(&name, &cancelled)?;
            let mut remote = read_leaf(&staging.parent, &name)?;
            let before = FileIdentity::read(&remote.metadata()?)?;
            source.verify_download(&mut remote, &cancelled)?;
            if FileIdentity::read(&remote.metadata()?)? != before
                || FileIdentity::read(&read_leaf(&staging.parent, &name)?.metadata()?)? != before
            {
                return Err(invalid("Remote copy changed during verification").into());
            }
            check(&cancelled)?;
            Ok(())
        })
    }
    pub fn download(
        &self,
        remote: PathBuf,
        destination: File,
        path: PathBuf,
    ) -> Result<Work<Result<Downloaded, TransferError>>, WorkError> {
        let root = self.root.clone();
        self.workers.submit_blocking(move |cancel| {
            let cancelled = || cancel.is_cancelled();
            check(&cancelled)?;
            let (source_parent, source_name) = parent(&root, &remote, false)?;
            let source = read_leaf(&source_parent, &source_name)?;
            let (parent, name) = parent(&destination, &path, true)?;
            if let Ok(existing) = read_leaf(&parent, &name) {
                let a = source.metadata()?;
                let b = existing.metadata()?;
                if a.dev() == b.dev() && a.ino() == b.ino() {
                    return Err(invalid("Source and destination are the same file").into());
                }
            }
            let mut staging = Staging::new(parent)?;
            let receipt = copy_complete(source, &mut staging.file, &cancelled)?;
            staging.publish(&name, &cancelled)?;
            Ok(receipt)
        })
    }
}
fn parent(root: &File, path: &Path, create: bool) -> io::Result<(File, OsString)> {
    let mut parts = Vec::new();
    for component in path.components() {
        match component {
            Component::Normal(name) => parts.push(name.to_owned()),
            _ => {
                return Err(invalid(
                    "Transfer path must be relative without dot components",
                ));
            }
        }
    }
    let name = parts
        .pop()
        .ok_or_else(|| invalid("Missing transfer filename"))?;
    let mut current = root.try_clone()?;
    for part in parts {
        if create {
            match mkdirat(&current, part.as_os_str(), Mode::from_bits_truncate(0o755)) {
                Ok(()) => current.sync_all()?,
                Err(nix::errno::Errno::EEXIST) => {}
                Err(error) => return Err(error.into()),
            }
        }
        current = File::from(openat(
            &current,
            part.as_os_str(),
            OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
            Mode::empty(),
        )?);
    }
    Ok((current, name))
}
fn read_leaf(parent: &File, name: &OsStr) -> io::Result<File> {
    let file = File::from(openat(
        parent,
        name,
        OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
        Mode::empty(),
    )?);
    FileIdentity::read(&file.metadata()?)?;
    Ok(file)
}
fn copy_complete(
    mut source: File,
    output: &mut File,
    cancel: &dyn Fn() -> bool,
) -> io::Result<Downloaded> {
    let metadata = source.metadata()?;
    let before = FileIdentity::read(&metadata)?;
    let receipt = copy_stream(&mut source, output, cancel)?;
    if receipt.length != before.length || FileIdentity::read(&source.metadata()?)? != before {
        return Err(invalid("Transfer source changed while copying"));
    }
    output.set_permissions(std::fs::Permissions::from_mode(metadata.mode() & 0o7777))?;
    futimens(
        &*output,
        &TimeSpec::new(metadata.atime(), metadata.atime_nsec()),
        &TimeSpec::new(metadata.mtime(), metadata.mtime_nsec()),
    )?;
    output.sync_all()?;
    check(cancel)?;
    Ok(receipt)
}

fn copy_stream(
    reader: &mut impl Read,
    output: &mut File,
    cancel: &dyn Fn() -> bool,
) -> io::Result<Downloaded> {
    let mut digest = Sha256::new();
    let mut length = 0u64;
    let mut buffer = [0u8; 65536];
    loop {
        check(cancel)?;
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        output.write_all(&buffer[..count])?;
        digest.update(&buffer[..count]);
        length = length
            .checked_add(count as u64)
            .ok_or_else(|| invalid("Transfer length overflow"))?;
    }
    check(cancel)?;
    Ok(Downloaded {
        length,
        digest: digest.finalize().into(),
    })
}

pub(super) fn receive_download(
    root: File,
    path: &Path,
    reader: &mut impl Read,
    expected: Option<u64>,
    cancel: &dyn Fn() -> bool,
) -> Result<Downloaded, TransferError> {
    check(cancel)?;
    let (parent, name) = parent(&root, path, true)?;
    let mut staging = Staging::new(parent)?;
    let receipt = copy_stream(reader, &mut staging.file, cancel)?;
    if expected.is_some_and(|length| length != receipt.length) {
        return Err(invalid("Remote download length mismatch").into());
    }
    staging.file.sync_all()?;
    staging.publish(&name, cancel)?;
    Ok(receipt)
}

struct Staging {
    parent: File,
    name: OsString,
    file: File,
    published: bool,
}
impl Staging {
    fn new(parent: File) -> io::Result<Self> {
        for _ in 0..8 {
            let mut random = [0u8; 16];
            getrandom::fill(&mut random)
                .map_err(|_| io::Error::other("Transfer staging randomness unavailable"))?;
            let token: String = random.iter().map(|b| format!("{b:02x}")).collect();
            let name = OsString::from(format!(".dockervault-transfer-{token}.partial"));
            match openat(
                &parent,
                name.as_os_str(),
                OFlag::O_RDWR
                    | OFlag::O_CREAT
                    | OFlag::O_EXCL
                    | OFlag::O_NOFOLLOW
                    | OFlag::O_CLOEXEC,
                Mode::from_bits_truncate(0o600),
            ) {
                Ok(fd) => {
                    return Ok(Self {
                        parent,
                        name,
                        file: File::from(fd),
                        published: false,
                    });
                }
                Err(nix::errno::Errno::EEXIST) => {}
                Err(error) => return Err(error.into()),
            }
        }
        Err(io::Error::new(
            io::ErrorKind::AlreadyExists,
            "Transfer staging collision",
        ))
    }
    fn publish(&mut self, name: &OsStr, cancel: &dyn Fn() -> bool) -> Result<(), TransferError> {
        check(cancel)?;
        renameat(&self.parent, self.name.as_os_str(), &self.parent, name)
            .map_err(io::Error::from)?;
        self.published = true;
        self.parent
            .sync_all()
            .map_err(TransferError::PublicationUncertain)?;
        Ok(())
    }
}
impl Drop for Staging {
    fn drop(&mut self) {
        if !self.published {
            let _ = unlinkat(
                &self.parent,
                self.name.as_os_str(),
                UnlinkatFlags::NoRemoveDir,
            );
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn published_staging_does_not_delete_a_new_occupant_of_its_old_name() {
        let mut token = [0u8; 16];
        getrandom::fill(&mut token).unwrap();
        let suffix: String = token.iter().map(|b| format!("{b:02x}")).collect();
        let directory = std::env::temp_dir().join(format!("dv-published-staging-{suffix}"));
        std::fs::create_dir(&directory).unwrap();
        let mut staging = Staging::new(File::open(&directory).unwrap()).unwrap();
        let old_name = staging.name.clone();
        staging.file.write_all(b"complete").unwrap();
        staging.file.sync_all().unwrap();
        staging.publish(OsStr::new("published"), &|| false).unwrap();
        std::fs::write(directory.join(&old_name), b"new unrelated file").unwrap();
        drop(staging);
        let result = std::fs::read(directory.join(old_name));
        std::fs::remove_dir_all(directory).unwrap();
        assert_eq!(result.unwrap(), b"new unrelated file");
    }
}
