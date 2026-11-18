//! Logical row insertion only. The bounded activation owner supplies an existing
//! transaction, claim, deployment cipher and configured backup root; this never
//! commits, deletes tables, probes providers or creates files/keys.
use crate::{
    recovery_records::{self, RecordValidationError},
    recovery_state::ValidatedState,
};
use RecordValidationError::{Cancelled, Invalid};
use dockervault_core::{crypto::CredentialCipher, portable_records::PortableTables};
use rusqlite::Transaction;
use serde::Deserialize;
use serde_json::value::RawValue;
use std::path::Path;
use tokio_util::sync::CancellationToken;

pub fn insert_state(
    transaction: &Transaction<'_>,
    state: &ValidatedState,
    backup_root: &Path,
    cipher: &CredentialCipher,
    cancel: &CancellationToken,
) -> Result<(), RecordValidationError> {
    if cancel.is_cancelled() {
        return Err(Cancelled);
    }
    if !transaction
        .pragma_query_value(None, "foreign_keys", |r| r.get::<_, bool>(0))
        .map_err(|_| Invalid)?
    {
        return Err(Invalid);
    }
    #[derive(Deserialize)]
    struct Packet<'a> {
        backup_root: String,
        #[serde(borrow)]
        tables: &'a RawValue,
    }
    let packet: Packet<'_> =
        serde_json::from_str(state.payload_json().as_str()).map_err(|_| Invalid)?;
    let mut records =
        PortableTables::parse_json(packet.tables.get().as_bytes()).map_err(|_| Invalid)?;
    for storage in &mut records.remote_storages {
        if cancel.is_cancelled() {
            return Err(Cancelled);
        }
        for value in [
            &mut storage.password,
            &mut storage.s3_access_key,
            &mut storage.s3_secret_key,
        ]
        .into_iter()
        .flatten()
        {
            *value = cipher.encrypt_value(value);
        }
    }
    for backup in &mut records.backups {
        if cancel.is_cancelled() {
            return Err(Cancelled);
        }
        for path in [&mut backup.file_path, &mut backup.encryption_key_path] {
            if let Some(path) = path
                && !path.is_empty()
            {
                *path = rebase(path, &packet.backup_root, backup_root)?;
            }
        }
    }
    recovery_records::insert_records(transaction, &records, cancel)
}
fn anchor(path: &str) -> &str {
    if path.starts_with("//") && !path.starts_with("///") {
        "//"
    } else if path.starts_with('/') {
        "/"
    } else {
        ""
    }
}
fn parts(path: &str) -> Vec<&str> {
    path.split('/')
        .filter(|v| !v.is_empty() && *v != ".")
        .collect()
}
fn rebase(path: &str, old_root: &str, new_root: &Path) -> Result<String, RecordValidationError> {
    let source = parts(path);
    let old = parts(old_root);
    if anchor(path) != anchor(old_root) || !source.starts_with(&old) || source.len() == old.len() {
        return Err(Invalid);
    }
    let new = new_root.to_str().ok_or(Invalid)?;
    let mut destination = parts(new);
    destination.extend(&source[old.len()..]);
    // Python PurePosixPath retains exactly two leading slashes and '..' in the
    // trusted configured root, but removes repeated separators and '.' segments.
    Ok(format!("{}{}", anchor(new), destination.join("/")))
}
