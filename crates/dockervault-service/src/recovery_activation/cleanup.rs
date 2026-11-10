use super::PreparationError;
use rusqlite::{Connection, OpenFlags, OptionalExtension, TransactionBehavior};
use serde_json::Value;
use sha2::{Digest, Sha256};
use std::{
    fs::{self, File, Metadata, OpenOptions},
    io::{self, Read},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt},
    },
    path::{Path, PathBuf},
};

pub(super) fn discard(database: &Path, key_file: &Path) -> Result<bool, PreparationError> {
    let mut connection = Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_WRITE)
        .map_err(|_| PreparationError::Io)?;
    connection
        .pragma_update(None, "foreign_keys", true)
        .map_err(|_| PreparationError::Io)?;
    let transaction = connection
        .transaction_with_behavior(TransactionBehavior::Immediate)
        .map_err(|_| PreparationError::Io)?;
    let users: i64 = transaction
        .query_row("SELECT count(*) FROM users", [], |row| row.get(0))
        .map_err(|_| PreparationError::Io)?;
    if users != 0 {
        return Ok(false);
    }
    let record: Option<Option<String>> = transaction
        .query_row(
            "SELECT value FROM app_settings WHERE key='recovery_activation'",
            [],
            |row| row.get(0),
        )
        .optional()
        .map_err(|_| PreparationError::Io)?;
    let Some(record) = record else {
        return Ok(false);
    };
    let value: Value =
        serde_json::from_str(record.as_deref().ok_or(PreparationError::InvalidJournal)?)
            .map_err(|_| PreparationError::InvalidJournal)?;
    if value.get("state").and_then(Value::as_str) != Some("preparing") {
        return Err(PreparationError::InvalidJournal);
    }
    let generation = value
        .get("generation")
        .and_then(Value::as_str)
        .filter(|s| {
            s.len() == 32
                && s.bytes()
                    .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b))
        })
        .ok_or(PreparationError::InvalidJournal)?;
    let key_preparing = match value.get("instance_key_preparing") {
        None => false,
        Some(value) => value.as_bool().ok_or(PreparationError::InvalidJournal)?,
    };
    remove_generation(database, generation)?;
    if key_preparing {
        remove_owned_key(key_file, generation, &value)?;
    }
    transaction
        .execute(
            "DELETE FROM app_settings WHERE key='recovery_activation'",
            [],
        )
        .map_err(|_| PreparationError::Io)?;
    transaction.commit().map_err(|_| PreparationError::Io)?;
    Ok(true)
}

fn directory(path: &Path) -> io::Result<File> {
    OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW)
        .open(path)
}
fn optional_metadata(path: &Path) -> Result<Option<Metadata>, PreparationError> {
    match fs::symlink_metadata(path) {
        Ok(metadata) => Ok(Some(metadata)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(_) => Err(PreparationError::Io),
    }
}
fn descriptor_path(directory: &File) -> Result<PathBuf, PreparationError> {
    // A missing procfs bridge must not look like a missing recovery generation.
    let path = PathBuf::from(format!("/proc/self/fd/{}", directory.as_raw_fd()));
    let held = directory.metadata().map_err(|_| PreparationError::Io)?;
    let resolved = fs::metadata(&path).map_err(|_| PreparationError::Io)?;
    if !same_identity(&held, &resolved) {
        return Err(PreparationError::Io);
    }
    Ok(path)
}
fn remove_generation(database: &Path, generation: &str) -> Result<(), PreparationError> {
    let root_path = database
        .parent()
        .ok_or(PreparationError::InvalidJournal)?
        .join(".recovery");
    let root = match directory(&root_path) {
        Ok(root) => root,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(_) => return Err(PreparationError::UnsafeGeneration),
    };
    let path = descriptor_path(&root)?.join(generation);
    if let Some(metadata) = optional_metadata(&path)? {
        if metadata.file_type().is_symlink() {
            return Err(PreparationError::UnsafeGeneration);
        }
        fs::remove_dir_all(path).map_err(|_| PreparationError::Io)?;
        root.sync_all().map_err(|_| PreparationError::Io)?;
    }
    Ok(())
}
fn same_identity(left: &Metadata, right: &Metadata) -> bool {
    left.dev() == right.dev() && left.ino() == right.ino()
}
fn remove_if_identity(path: &Path, expected: &Metadata) -> Result<(), PreparationError> {
    let current = optional_metadata(path)?.ok_or(PreparationError::UnsafeKeyPreparation)?;
    if !same_identity(&current, expected) {
        return Err(PreparationError::UnsafeKeyPreparation);
    }
    fs::remove_file(path).map_err(|_| PreparationError::Io)
}
fn remove_owned_key(
    key_file: &Path,
    generation: &str,
    record: &Value,
) -> Result<(), PreparationError> {
    let parent = directory(key_file.parent().ok_or(PreparationError::InvalidJournal)?)
        .map_err(|_| PreparationError::Io)?;
    let root = descriptor_path(&parent)?;
    let key = root.join(
        key_file
            .file_name()
            .ok_or(PreparationError::InvalidJournal)?,
    );
    let stage = root.join(format!(".dv-recovery-key-{generation}"));
    let staged = optional_metadata(&stage)?;
    if staged.as_ref().is_some_and(|info| !info.is_file()) {
        return Err(PreparationError::UnsafeKeyPreparation);
    }
    let active = optional_metadata(&key)?;
    let owned = match active.as_ref().filter(|info| info.is_file()) {
        None => false,
        Some(active) => match &staged {
            Some(staged) => same_identity(active, staged),
            None => match record.get("key_digest") {
                None => false,
                Some(digest) => {
                    let digest = digest.as_str().ok_or(PreparationError::InvalidJournal)?;
                    let inode = record
                        .get("key_inode")
                        .and_then(Value::as_u64)
                        .ok_or(PreparationError::InvalidJournal)?;
                    let device = record
                        .get("key_device")
                        .and_then(Value::as_u64)
                        .ok_or(PreparationError::InvalidJournal)?;
                    let file = OpenOptions::new()
                        .read(true)
                        .custom_flags(nix::libc::O_NOFOLLOW)
                        .open(&key)
                        .map_err(|_| PreparationError::Io)?;
                    let held = file.metadata().map_err(|_| PreparationError::Io)?;
                    let mut bytes = Vec::new();
                    file.take(1024)
                        .read_to_end(&mut bytes)
                        .map_err(|_| PreparationError::Io)?;
                    let actual: String = Sha256::digest(&bytes)
                        .iter()
                        .map(|b| format!("{b:02x}"))
                        .collect();
                    same_identity(active, &held)
                        && held.ino() == inode
                        && held.dev() == device
                        && actual == digest
                }
            },
        },
    };
    if owned {
        remove_if_identity(&key, active.as_ref().expect("owned active file"))?;
    }
    if let Some(staged) = staged {
        remove_if_identity(&stage, &staged)?;
    }
    parent.sync_all().map_err(|_| PreparationError::Io)?;
    Ok(())
}
