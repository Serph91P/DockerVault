//! Physically owned, offline recovery validation. No live database/default-key
//! setup, writes, network checks or activation. Success is a preview, not import.
use crate::{
    ownership::Owner,
    recovery_records::{self, RecordValidationError},
};
use dockervault_core::{
    cron::TimeZone,
    lossless_json::PythonJson,
    recovery_state::{StateError, parse_state, parse_state_json},
    timestamp::Timestamp,
};
use nix::{
    errno::Errno,
    fcntl::{OFlag, open, openat},
    sys::stat::{Mode, SFlag, fstat},
};
use serde_json::Value;
use std::{
    collections::{BTreeMap, BTreeSet},
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

// Explicit trusted runtime inputs, never sourced from the portable packet.
pub struct ValidationContext {
    pub backup_root: PathBuf,
    pub now: Timestamp,
    pub timezone: TimeZone,
    pub rclone_options: BTreeMap<String, BTreeSet<String>>,
}
pub struct ValidatedState {
    payload: PythonJson,
    preview: Value,
}
impl ValidatedState {
    pub fn payload(&self) -> Result<Value, RecordValidationError> {
        // Scalar-only callers get an explicit error, never replacement data.
        serde_json::from_str(self.payload.as_str()).map_err(|_| RecordValidationError::Invalid)
    }
    pub fn payload_json(&self) -> &PythonJson {
        &self.payload
    }
    pub fn preview(&self) -> &Value {
        &self.preview
    }
}
fn missing(error: Errno) -> bool {
    matches!(error, Errno::ENOENT | Errno::ENOTDIR)
}
fn kind(fd: &impl std::os::fd::AsFd) -> Result<SFlag, RecordValidationError> {
    fstat(fd)
        .map(|s| SFlag::from_bits_truncate(s.st_mode) & SFlag::S_IFMT)
        .map_err(|_| RecordValidationError::Invalid)
}
fn check_paths(
    root: &Path,
    paths: &[Vec<String>],
    cancel: &CancellationToken,
) -> Result<(), RecordValidationError> {
    if paths.is_empty() {
        return Ok(());
    }
    // O_PATH never opens FIFO/device data; NOFOLLOW plus fstat detects symlink
    // leaves rather than treating ELOOP/ENOTDIR as an absent legitimate path.
    let flags = OFlag::O_PATH | OFlag::O_CLOEXEC | OFlag::O_NOFOLLOW;
    let root = match open(root, flags, Mode::empty()) {
        Ok(fd) => fd,
        Err(e) if missing(e) => return Ok(()),
        Err(_) => return Err(RecordValidationError::Invalid),
    };
    let mode = kind(&root)?;
    if mode == SFlag::S_IFLNK {
        return Err(RecordValidationError::Invalid);
    }
    if mode != SFlag::S_IFDIR {
        return Ok(());
    }
    for parts in paths {
        let mut current = nix::unistd::dup(&root).map_err(|_| RecordValidationError::Invalid)?;
        for part in parts {
            if cancel.is_cancelled() {
                return Err(RecordValidationError::Cancelled);
            }
            let next = match openat(&current, part.as_str(), flags, Mode::empty()) {
                Ok(fd) => fd,
                Err(e) if missing(e) => break,
                Err(_) => return Err(RecordValidationError::Invalid),
            };
            let mode = kind(&next)?;
            if mode == SFlag::S_IFLNK {
                return Err(RecordValidationError::Invalid);
            }
            if mode != SFlag::S_IFDIR {
                break;
            }
            current = next;
        }
    }
    Ok(())
}
pub async fn validate_state(
    payload: Value,
    context: ValidationContext,
    owner: &Owner,
) -> Result<ValidatedState, RecordValidationError> {
    validate_input(Input::Value(payload), context, owner).await
}
pub async fn validate_state_json(
    payload: Vec<u8>,
    context: ValidationContext,
    owner: &Owner,
) -> Result<ValidatedState, RecordValidationError> {
    validate_input(Input::Bytes(payload), context, owner).await
}
enum Input {
    Value(Value),
    Bytes(Vec<u8>),
}
async fn validate_input(
    payload: Input,
    context: ValidationContext,
    owner: &Owner,
) -> Result<ValidatedState, RecordValidationError> {
    owner
        .submit_blocking(move |cancel| {
            let parsed = match payload {
                Input::Value(payload) => parse_state(
                    payload,
                    context.now,
                    &context.timezone,
                    &context.rclone_options,
                    &|| cancel.is_cancelled(),
                ),
                Input::Bytes(payload) => PythonJson::parse(&payload)
                    .map_err(|_| StateError::Invalid)
                    .and_then(|document| {
                        parse_state_json(
                            document,
                            context.now,
                            &context.timezone,
                            &context.rclone_options,
                            &|| cancel.is_cancelled(),
                        )
                    }),
            }
            .map_err(|e| {
                if e == StateError::Cancelled {
                    RecordValidationError::Cancelled
                } else {
                    RecordValidationError::Invalid
                }
            })?;
            check_paths(&context.backup_root, &parsed.backup_paths, &cancel).map_err(|e| {
                if cancel.is_cancelled() {
                    RecordValidationError::Cancelled
                } else {
                    e
                }
            })?;
            // Reuse the same physical worker/permit; recursively submitting with a
            // one-slot owner would deadlock and could abandon relational staging.
            recovery_records::check(&parsed.tables, &cancel).map_err(|e| {
                if cancel.is_cancelled() {
                    RecordValidationError::Cancelled
                } else {
                    e
                }
            })?;
            if cancel.is_cancelled() {
                return Err(RecordValidationError::Cancelled);
            }
            Ok(ValidatedState {
                payload: parsed.payload,
                preview: parsed.preview,
            })
        })
        .map_err(|_| RecordValidationError::Owner)?
        .wait()
        .await
        .map_err(|_| RecordValidationError::Owner)?
}
