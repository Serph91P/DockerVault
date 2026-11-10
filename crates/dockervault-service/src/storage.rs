//! Invocation-local source identity and provider byte-verification primitives.
pub mod local;
pub mod webdav;
use sha2::{Digest, Sha256};
use std::{
    fs::{File, Metadata, OpenOptions},
    io::{self, Read},
    os::unix::fs::{MetadataExt, OpenOptionsExt},
    path::{Path, PathBuf},
};

#[derive(Clone, Copy, PartialEq, Eq)]
struct FileIdentity {
    device: u64,
    inode: u64,
    length: u64,
    modified: (i64, i64),
    changed: (i64, i64),
}
impl FileIdentity {
    fn read(metadata: &Metadata) -> io::Result<Self> {
        if !metadata.is_file() {
            return Err(invalid("Transfer source is not a regular file"));
        }
        Ok(Self {
            device: metadata.dev(),
            inode: metadata.ino(),
            length: metadata.len(),
            modified: (metadata.mtime(), metadata.mtime_nsec()),
            changed: (metadata.ctime(), metadata.ctime_nsec()),
        })
    }
}

/// Not a cache: retain only for one transfer invocation, including every destination/sidecar.
/// The open descriptor prevents inode recycling while this receipt exists.
pub struct VerifiedSource {
    path: PathBuf,
    file: File,
    identity: FileIdentity,
    digest: [u8; 32],
}
impl std::fmt::Debug for VerifiedSource {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VerifiedSource")
            .field("length", &self.len())
            .finish_non_exhaustive()
    }
}
impl VerifiedSource {
    pub fn open(path: &Path, cancel: &dyn Fn() -> bool) -> io::Result<Self> {
        check(cancel)?;
        let mut file = open_regular(path)?;
        let identity = FileIdentity::read(&file.metadata()?)?;
        let digest = hash_reader(&mut file, cancel)?;
        let source = Self {
            path: path.to_owned(),
            file,
            identity,
            digest,
        };
        source.check_unchanged()?;
        Ok(source)
    }
    pub fn len(&self) -> u64 {
        self.identity.length
    }
    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
    pub fn sha256_hex(&self) -> String {
        self.digest
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect()
    }
    pub fn check_unchanged(&self) -> io::Result<()> {
        if FileIdentity::read(&self.file.metadata()?)? != self.identity
            || FileIdentity::read(&std::fs::symlink_metadata(&self.path)?)? != self.identity
        {
            return Err(invalid("Transfer source changed during invocation"));
        }
        Ok(())
    }
    pub fn verify_download(
        &self,
        reader: &mut impl Read,
        cancel: &dyn Fn() -> bool,
    ) -> io::Result<()> {
        self.check_unchanged()?;
        let actual = hash_reader(reader, cancel)?;
        self.check_unchanged()?;
        if actual != self.digest {
            return Err(invalid("Independent remote byte verification failed"));
        }
        Ok(())
    }
    /// Independent file descriptions start every retry at byte zero, not a shared dup offset.
    pub fn reopen(&self) -> io::Result<File> {
        self.check_unchanged()?;
        let file = open_regular(&self.path)?;
        if FileIdentity::read(&file.metadata()?)? != self.identity {
            return Err(invalid("Transfer source replaced while opening retry"));
        }
        Ok(file)
    }
}
fn open_regular(path: &Path) -> io::Result<File> {
    let file = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
        .open(path)?;
    FileIdentity::read(&file.metadata()?)?;
    Ok(file)
}
fn hash_reader(reader: &mut impl Read, cancel: &dyn Fn() -> bool) -> io::Result<[u8; 32]> {
    let mut digest = Sha256::new();
    let mut buffer = [0u8; 65536];
    loop {
        check(cancel)?;
        let count = reader.read(&mut buffer)?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    check(cancel)?;
    Ok(digest.finalize().into())
}
fn invalid(message: &'static str) -> io::Error {
    io::Error::new(io::ErrorKind::InvalidData, message)
}
fn check(cancel: &dyn Fn() -> bool) -> io::Result<()> {
    if cancel() {
        Err(io::Error::new(
            io::ErrorKind::Interrupted,
            "Transfer cancelled",
        ))
    } else {
        Ok(())
    }
}
