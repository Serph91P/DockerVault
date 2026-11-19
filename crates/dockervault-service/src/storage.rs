//! Invocation-local source identity and provider byte-verification primitives.
pub mod ftp;
pub mod local;
pub mod s3;
pub mod ssh;
mod sync;
pub(crate) use sync::{LocalCopy, complete_publication, settle_local_copies};
mod upload;
pub mod webdav;

pub(crate) fn relative_path(path: &str, allow_root: bool) -> Option<String> {
    if path.chars().count() > 1024
        || path.contains(['\0', '\\'])
        || path.starts_with('/')
        || path
            .as_bytes()
            .get(..2)
            .is_some_and(|p| p[0].is_ascii_alphabetic() && p[1] == b':')
        || path.split('/').any(|part| part == "..")
    {
        return None;
    }
    let normalized = path
        .split('/')
        .filter(|part| !matches!(*part, "" | "."))
        .collect::<Vec<_>>()
        .join("/");
    (allow_root || !normalized.is_empty()).then_some(normalized)
}
use sha2::{Digest, Sha256};

const PUBLICATION_ATTR: &str = "user.dockervault.publication";

#[derive(serde::Serialize, serde::Deserialize)]
pub(crate) struct Publication {
    pub token: String,
    pub checksum: String,
    pub artifacts: Vec<PublishedArtifact>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub local_copies: Vec<LocalCopy>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub delete_local_after_sync: bool,
}

#[derive(Clone, serde::Serialize, serde::Deserialize)]
pub(crate) struct PublishedArtifact {
    pub path: PathBuf,
    pub length: u64,
    pub digest: String,
    pub device: u64,
    pub inode: u64,
}

pub(crate) fn mark_publication(file: &File, token: &str) -> io::Result<()> {
    rustix::fs::fsetxattr(
        file,
        PUBLICATION_ATTR,
        token.as_bytes(),
        rustix::fs::XattrFlags::CREATE,
    )?;
    file.sync_all()
}

fn digest_hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn verified_publication(
    root: &Path,
    paths: &[PathBuf],
    proof: &Publication,
    cancel: &dyn Fn() -> bool,
) -> bool {
    let valid_hex = |s: &str, len| s.len() == len && s.bytes().all(|b| b.is_ascii_hexdigit());
    if !valid_hex(&proof.token, 32)
        || !valid_hex(&proof.checksum, 64)
        || paths.len() != proof.artifacts.len()
    {
        return false;
    }
    let Some(archive) = paths.first().and_then(|path| path.to_str()) else {
        return false;
    };
    match paths {
        [_] if !archive.ends_with(".enc") && proof.checksum == proof.artifacts[0].digest => {}
        [_, key]
            if archive.ends_with(".enc")
                && key == Path::new(&dockervault_core::crypto::key_sidecar_path(archive)) => {}
        _ => return false,
    }
    paths.iter().zip(&proof.artifacts).all(|(path, artifact)| {
        if cancel() || *path != artifact.path || !valid_hex(&artifact.digest, 64) {
            return false;
        }
        let check = || -> io::Result<bool> {
            let rel = path.strip_prefix(root).map_err(io::Error::other)?;
            let mut file = local::open_download_source(root, rel)?;
            let identity = FileIdentity::read(&file.metadata()?)?;
            let mut token = [0u8; 32];
            if identity.length != artifact.length
                || identity.device != artifact.device
                || identity.inode != artifact.inode
                || rustix::fs::fgetxattr(&file, PUBLICATION_ATTR, &mut token[..])? != token.len()
                || token.as_slice() != proof.token.as_bytes()
            {
                return Ok(false);
            }
            if digest_hex(&hash_reader(&mut file, cancel)?) != artifact.digest {
                return Ok(false);
            }
            let current = local::open_download_source(root, rel)?;
            if identity != FileIdentity::read(&current.metadata()?)? {
                return Ok(false);
            }
            file.sync_all()?;
            local::open_root(
                path.parent()
                    .ok_or_else(|| invalid("Missing publication parent"))?,
            )?
            .sync_all()?;
            Ok(true)
        };
        check().unwrap_or(false)
    })
}

/// Reconcile native publications before opening the listener. Never remove a
/// filesystem occupant based on a reservation alone.
pub async fn reconcile_archive_publications(
    pool: &sqlx::SqlitePool,
    root: &str,
    owner: &crate::ownership::Owner,
) -> Result<(), sqlx::Error> {
    if root.is_empty() {
        return Ok(());
    }
    let root = std::path::absolute(root).map_err(sqlx::Error::Io)?;
    let mut after = i64::MIN;
    loop {
        let rows: Vec<(i64, String, String)> = sqlx::query_as(
            "SELECT id,status,backup_metadata FROM backups WHERE id>? AND status IN ('PENDING','RUNNING','FAILED')
             AND CASE WHEN json_valid(backup_metadata) THEN json_type(backup_metadata,'$.native_ephemeral_archive')='true' ELSE 0 END
             ORDER BY id LIMIT 64",
        ).bind(after).fetch_all(pool).await?;
        if rows.is_empty() {
            return Ok(());
        }
        for (id, status, metadata) in rows {
            after = id;
            let Ok(metadata_value) = serde_json::from_str::<serde_json::Value>(&metadata) else {
                continue;
            };
            let Some(paths) = metadata_value
                .get("active_artifacts")
                .and_then(|paths| serde_json::from_value::<Vec<PathBuf>>(paths.clone()).ok())
            else {
                continue;
            };
            if !(1..=2).contains(&paths.len()) {
                continue;
            }
            let proof = metadata_value
                .get("native_publication")
                .and_then(|value| serde_json::from_value::<Publication>(value.clone()).ok());
            let root = root.clone();
            let (confirmed_absent, verified) = owner.submit_blocking(move |cancel| {
                let absent = paths.iter().all(|path| {
                    !cancel.is_cancelled() && path.strip_prefix(&root).ok().is_some_and(|rel| {
                        !rel.as_os_str().is_empty() && rel.components().all(|part| matches!(part,std::path::Component::Normal(_)))
                            && matches!(local::open_download_source(&root, rel), Err(error) if error.kind() == io::ErrorKind::NotFound)
                    })
                });
                let verified = proof.filter(|proof| status == "RUNNING" && !absent && verified_publication(&root, &paths, proof, &|| cancel.is_cancelled()));
                (absent, verified)
            }).map_err(|e|sqlx::Error::Io(io::Error::other(e)))?.wait().await.map_err(|e|sqlx::Error::Io(io::Error::other(e)))?;
            if let Some(proof) = verified {
                complete_publication(pool, id, &proof).await?;
            } else if confirmed_absent {
                sqlx::query("UPDATE backups SET backup_metadata=json_remove(backup_metadata,'$.active_artifacts','$.native_ephemeral_archive','$.native_publication') WHERE id=? AND backup_metadata=? AND status IN ('PENDING','RUNNING','FAILED')")
                    .bind(id).bind(metadata).execute(pool).await?;
            } else {
                sqlx::query("UPDATE backups SET status='FAILED', error_message=?,
                    completed_at=coalesce(completed_at,strftime('%Y-%m-%d %H:%M:%f000','now'))
                    WHERE id=? AND backup_metadata=? AND status IN ('PENDING','RUNNING')")
                    .bind("Interrupted backup: publication ownership unconfirmed. Reserved paths were preserved; verify them before recovery.")
                    .bind(id).bind(metadata).execute(pool).await?;
            }
        }
    }
}
pub async fn reconcile_backup_storage(
    pool: &sqlx::SqlitePool,
    root: &str,
    owner: &crate::ownership::Owner,
) -> Result<(), sqlx::Error> {
    reconcile_archive_publications(pool, root, owner).await?;
    sync::reconcile_local_copies(pool, owner).await
}

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
pub(crate) fn hash_reader(
    reader: &mut impl Read,
    cancel: &dyn Fn() -> bool,
) -> io::Result<[u8; 32]> {
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
