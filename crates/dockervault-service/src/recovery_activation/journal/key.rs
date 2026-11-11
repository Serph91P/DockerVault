//! Instance-key preparation under the durable generation and physical setup claim.
use super::{JournalError, RecoveryGeneration};
use dockervault_core::crypto::CredentialCipher;
use nix::{
    errno::Errno,
    fcntl::{AtFlags, OFlag, openat},
    sys::stat::{FileStat, Mode, fstatat, mkdirat},
    unistd::{UnlinkatFlags, linkat, unlinkat},
};
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{
    ffi::OsStr,
    fs::{File, OpenOptions, Permissions},
    io::{Read, Seek, SeekFrom, Write},
    os::{
        fd::AsRawFd,
        unix::fs::{MetadataExt, OpenOptionsExt, PermissionsExt},
    },
    path::{Component, Path},
};
use tokio_util::sync::CancellationToken;

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum KeyPreparation {
    Configured,
    Existing,
    Created,
}
fn check(cancel: &CancellationToken) -> Result<(), JournalError> {
    if cancel.is_cancelled() {
        Err(JournalError::Cancelled)
    } else {
        Ok(())
    }
}
fn leaf(parent: &File, name: &OsStr) -> Result<Option<FileStat>, JournalError> {
    match fstatat(parent, name, AtFlags::AT_SYMLINK_NOFOLLOW) {
        Ok(info) => Ok(Some(info)),
        Err(Errno::ENOENT) => Ok(None),
        Err(_) => Err(JournalError::Io),
    }
}
fn parent(
    path: &Path,
    create: bool,
    cancel: &CancellationToken,
) -> Result<Option<File>, JournalError> {
    let mut current = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW | nix::libc::O_CLOEXEC)
        .open("/")
        .map_err(|_| JournalError::Io)?;
    for part in path.components() {
        check(cancel)?;
        let name = match part {
            Component::RootDir | Component::CurDir => continue,
            Component::Normal(name) => name,
            Component::ParentDir => OsStr::new(".."),
            _ => return Err(JournalError::UnsafeKeyPreparation),
        };
        let flags = OFlag::O_RDONLY | OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
        current = match openat(&current, name, flags, Mode::empty()) {
            Ok(next) => File::from(next),
            Err(Errno::ENOENT) if !create => return Ok(None),
            Err(Errno::ENOENT) => {
                match mkdirat(&current, name, Mode::from_bits_truncate(0o700)) {
                    Ok(()) => current.sync_all().map_err(|_| JournalError::Io)?,
                    Err(Errno::EEXIST) => {}
                    Err(_) => return Err(JournalError::Io),
                }
                File::from(
                    openat(&current, name, flags, Mode::empty())
                        .map_err(|_| JournalError::UnsafeKeyPreparation)?,
                )
            }
            Err(Errno::ELOOP | Errno::ENOTDIR) => return Err(JournalError::UnsafeKeyPreparation),
            Err(_) => return Err(JournalError::Io),
        };
    }
    Ok(Some(current))
}
impl RecoveryGeneration<'_> {
    fn key_journal(&self, fields: Value) -> Result<(), JournalError> {
        let path = self
            .guard
            .database
            .as_ref()
            .ok_or(JournalError::NotDurable)?;
        let mut connection = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_WRITE)
            .map_err(|_| JournalError::Io)?;
        connection
            .pragma_update(None, "foreign_keys", true)
            .map_err(|_| JournalError::Io)?;
        let transaction = connection
            .transaction_with_behavior(TransactionBehavior::Immediate)
            .map_err(|_| JournalError::Io)?;
        let users: i64 = transaction
            .query_row("SELECT count(*) FROM users", [], |row| row.get(0))
            .map_err(|_| JournalError::Io)?;
        if users != 0 {
            return Err(JournalError::AlreadySetup);
        }
        let text: String = transaction
            .query_row(
                "SELECT value FROM app_settings WHERE key='recovery_activation'",
                [],
                |row| row.get(0),
            )
            .map_err(|_| JournalError::InvalidJournal)?;
        let mut record: Value =
            serde_json::from_str(&text).map_err(|_| JournalError::InvalidJournal)?;
        if record.get("state").and_then(Value::as_str) != Some("preparing")
            || record.get("generation").and_then(Value::as_str) != Some(self.generation.as_str())
        {
            return Err(JournalError::InvalidJournal);
        }
        record
            .as_object_mut()
            .ok_or(JournalError::InvalidJournal)?
            .extend(
                fields
                    .as_object()
                    .ok_or(JournalError::InvalidJournal)?
                    .clone(),
            );
        transaction
            .execute(
                "UPDATE app_settings SET value=? WHERE key='recovery_activation'",
                [record.to_string()],
            )
            .map_err(|_| JournalError::Io)?;
        transaction.commit().map_err(|_| JournalError::Io)
    }
    /// No lazy/global key loader: journal ownership before publishing a new private
    /// key. Keep this generation's guard in the physical owner through all returns.
    pub fn prepare_instance_key(
        &self,
        key_file: &Path,
        configured_key: bool,
        cancel: &CancellationToken,
    ) -> Result<KeyPreparation, JournalError> {
        check(cancel)?;
        if configured_key {
            return Ok(KeyPreparation::Configured);
        }
        let path = std::path::absolute(key_file).map_err(|_| JournalError::Io)?;
        let name = path.file_name().ok_or(JournalError::UnsafeKeyPreparation)?;
        let directory = path.parent().ok_or(JournalError::UnsafeKeyPreparation)?;
        let stage = format!(".dv-recovery-key-{}", self.generation);
        let existing_parent = parent(directory, false, cancel)?;
        if let Some(parent) = &existing_parent {
            if let Some(info) = leaf(parent, name)? {
                if info.st_mode & nix::libc::S_IFMT != nix::libc::S_IFREG {
                    return Err(JournalError::UnsafeKeyPreparation);
                }
                return Ok(KeyPreparation::Existing);
            }
            if leaf(parent, OsStr::new(&stage))?.is_some() {
                return Err(JournalError::UnsafeKeyPreparation);
            }
        }
        check(cancel)?;
        self.key_journal(json!({"instance_key_preparing":true}))?;
        check(cancel)?;
        let parent = match existing_parent {
            Some(parent) => parent,
            None => parent(directory, true, cancel)?.ok_or(JournalError::Io)?,
        };
        let mut file = File::from(
            openat(
                &parent,
                stage.as_str(),
                OFlag::O_RDWR
                    | OFlag::O_CREAT
                    | OFlag::O_EXCL
                    | OFlag::O_NOFOLLOW
                    | OFlag::O_CLOEXEC,
                Mode::from_bits_truncate(0o600),
            )
            .map_err(|_| JournalError::UnsafeKeyPreparation)?,
        );
        file.set_permissions(Permissions::from_mode(0o600))
            .map_err(|_| JournalError::Io)?;
        let key = CredentialCipher::generate_key();
        file.write_all(key.as_bytes())
            .map_err(|_| JournalError::Io)?;
        file.sync_all().map_err(|_| JournalError::Io)?;
        let held = file.metadata().map_err(|_| JournalError::Io)?;
        let mut readback = key.clone();
        readback.clear();
        file.seek(SeekFrom::Start(0))
            .map_err(|_| JournalError::Io)?;
        (&mut file)
            .take(1024)
            .read_to_string(&mut readback)
            .map_err(|_| JournalError::Io)?;
        if readback.as_bytes() != key.as_bytes() {
            return Err(JournalError::UnsafeKeyPreparation);
        }
        check(cancel)?;
        let staged =
            leaf(&parent, OsStr::new(&stage))?.ok_or(JournalError::UnsafeKeyPreparation)?;
        if staged.st_dev != held.dev() || staged.st_ino != held.ino() {
            return Err(JournalError::UnsafeKeyPreparation);
        }
        // Publish the held file, not a staging pathname another actor could reuse.
        let descriptor = format!("/proc/self/fd/{}", file.as_raw_fd());
        let resolved = std::fs::metadata(&descriptor).map_err(|_| JournalError::Io)?;
        if resolved.dev() != held.dev() || resolved.ino() != held.ino() {
            return Err(JournalError::UnsafeKeyPreparation);
        }
        linkat(
            &parent,
            descriptor.as_str(),
            &parent,
            name,
            AtFlags::AT_SYMLINK_FOLLOW,
        )
        .map_err(|_| JournalError::UnsafeKeyPreparation)?;
        let published = leaf(&parent, name)?.ok_or(JournalError::UnsafeKeyPreparation)?;
        if published.st_dev != held.dev() || published.st_ino != held.ino() {
            return Err(JournalError::UnsafeKeyPreparation);
        }
        parent.sync_all().map_err(|_| JournalError::Io)?;
        let digest: String = Sha256::digest(key.as_bytes())
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect();
        // After publication settle durable ownership even if the waiter cancels.
        self.key_journal(
            json!({"key_digest":digest,"key_inode":held.ino(),"key_device":held.dev()}),
        )?;
        let staged =
            leaf(&parent, OsStr::new(&stage))?.ok_or(JournalError::UnsafeKeyPreparation)?;
        if staged.st_dev != held.dev() || staged.st_ino != held.ino() {
            return Err(JournalError::UnsafeKeyPreparation);
        }
        unlinkat(&parent, stage.as_str(), UnlinkatFlags::NoRemoveDir)
            .map_err(|_| JournalError::Io)?;
        parent.sync_all().map_err(|_| JournalError::Io)?;
        check(cancel)?;
        Ok(KeyPreparation::Created)
    }
}
