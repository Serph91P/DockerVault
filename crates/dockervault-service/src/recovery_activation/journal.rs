//! Durable first-run admission. All synchronous calls belong in the physical owner.
use super::{ClaimKind, PreparationGuard};
use rusqlite::{Connection, OpenFlags, TransactionBehavior};
use serde::{
    Deserialize, Deserializer,
    de::{MapAccess, Visitor},
};
use serde_json::value::RawValue;
use sqlx::SqlitePool;
use std::{fmt, path::PathBuf};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum JournalError {
    Io,
    AlreadySetup,
    WrongClaim,
    NotDurable,
    InvalidJournal,
    UnsafeGeneration,
    Pending,
}
impl fmt::Display for JournalError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(match self {
            Self::Io => "Recovery preparation failed",
            Self::AlreadySetup => "Setup already completed. Use login instead.",
            Self::WrongClaim => "Recovery preparation requires the setup claim",
            Self::NotDurable => "Durable recovery requires a file-backed database",
            Self::InvalidJournal => "Recovery preparation journal needs repair",
            Self::UnsafeGeneration => "Unsafe recovery generation",
            Self::Pending => "System recovery setup is in progress",
        })
    }
}
impl std::error::Error for JournalError {}

// Last decoded state wins, as in the provider's journal json.loads (unlike its
// strict portable-envelope parser). Unknown opaque fields do not affect readiness.
struct Active(bool);
struct Field(bool);
impl<'de> Deserialize<'de> for Field {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct Key;
        impl Visitor<'_> for Key {
            type Value = Field;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("journal key")
            }
            fn visit_bytes<E: serde::de::Error>(self, v: &[u8]) -> Result<Field, E> {
                Ok(Field(v == b"state"))
            }
            fn visit_byte_buf<E: serde::de::Error>(self, v: Vec<u8>) -> Result<Field, E> {
                self.visit_bytes(&v)
            }
        }
        d.deserialize_byte_buf(Key)
    }
}
impl<'de> Deserialize<'de> for Active {
    fn deserialize<D: Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        struct State;
        impl<'de> Visitor<'de> for State {
            type Value = Active;
            fn expecting(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                f.write_str("journal object")
            }
            fn visit_map<A: MapAccess<'de>>(self, mut map: A) -> Result<Active, A::Error> {
                let mut active = false;
                while let Some(Field(state)) = map.next_key::<Field>()? {
                    let raw = map.next_value::<Box<RawValue>>()?;
                    if state {
                        active =
                            serde_json::from_str::<String>(raw.get()).is_ok_and(|v| v == "active");
                    }
                }
                Ok(Active(active))
            }
        }
        d.deserialize_map(State)
    }
}
// Readiness only: Python's journal json.loads accepts NaN/Infinity and bounds
// integer tokens. Project non-string constants to null without changing the DB
// or relaxing the distinct strict portable-envelope JSON parser.
fn readiness_projection(text: &str) -> Option<Vec<u8>> {
    let input = text.as_bytes();
    let mut output = Vec::with_capacity(input.len());
    let mut cursor = 0;
    let mut string = false;
    let mut escaped = false;
    while cursor < input.len() {
        let byte = input[cursor];
        if string {
            output.push(byte);
            cursor += 1;
            if escaped {
                escaped = false;
            } else if byte == b'\\' {
                escaped = true;
            } else if byte == b'"' {
                string = false;
            }
            continue;
        }
        if byte == b'"' {
            string = true;
            output.push(byte);
            cursor += 1;
            continue;
        }
        if let Some(token) = [b"-Infinity".as_slice(), b"Infinity", b"NaN"]
            .into_iter()
            .find(|token| input[cursor..].starts_with(token))
        {
            output.extend_from_slice(b"null");
            cursor += token.len();
            continue;
        }
        if byte == b'-' || byte.is_ascii_digit() {
            let start = cursor;
            while cursor < input.len()
                && !matches!(
                    input[cursor],
                    b' ' | b'\t' | b'\r' | b'\n' | b',' | b']' | b'}'
                )
            {
                cursor += 1;
            }
            let token = &input[start..cursor];
            let digits = token.strip_prefix(b"-").unwrap_or(token);
            if digits.len() > 4300 && digits.iter().all(u8::is_ascii_digit) {
                return None;
            }
            output.extend_from_slice(token);
            continue;
        }
        output.push(byte);
        cursor += 1;
    }
    Some(output)
}
pub async fn recovery_pending(pool: &SqlitePool) -> Result<bool, JournalError> {
    let value: Option<Option<String>> =
        sqlx::query_scalar("SELECT value FROM app_settings WHERE key='recovery_activation'")
            .fetch_optional(pool)
            .await
            .map_err(|_| JournalError::Io)?;
    Ok(value.flatten().is_some_and(|value| {
        !readiness_projection(&value)
            .is_some_and(|json| serde_json::from_slice::<Active>(&json).is_ok_and(|v| v.0))
    }))
}
pub async fn require_recovery_ready(pool: &SqlitePool) -> Result<(), JournalError> {
    if recovery_pending(pool).await? {
        Err(JournalError::Pending)
    } else {
        Ok(())
    }
}

// A generation cannot outlive its exclusive setup claim. Dropping it does NOT
// clear the durable marker; restart cleanup must prove and settle owned effects.
pub struct RecoveryGeneration<'claim> {
    guard: &'claim PreparationGuard,
    generation: String,
}
impl RecoveryGeneration<'_> {
    pub fn identifier(&self) -> &str {
        &self.generation
    }
    /// Path projection only; subsequent filesystem effects need held no-follow authority.
    pub fn directory(&self) -> Result<PathBuf, JournalError> {
        let root = self
            .guard
            .database
            .as_ref()
            .and_then(|p| p.parent())
            .ok_or(JournalError::NotDurable)?
            .join(".recovery");
        match std::fs::symlink_metadata(&root) {
            Ok(info) if !info.is_dir() || info.file_type().is_symlink() => {
                return Err(JournalError::UnsafeGeneration);
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => {
                return Err(JournalError::Io);
            }
            _ => {}
        }
        Ok(root.join(&self.generation))
    }
}
impl PreparationGuard {
    /// Commit the preparation marker before creating any generation/key material.
    /// The caller retains this guard in its physical blocking owner through settlement.
    pub fn begin_recovery(&self) -> Result<RecoveryGeneration<'_>, JournalError> {
        if !matches!(self.kind, ClaimKind::Setup) {
            return Err(JournalError::WrongClaim);
        }
        let database = self.database.as_ref().ok_or(JournalError::NotDurable)?;
        let mut connection =
            Connection::open_with_flags(database, OpenFlags::SQLITE_OPEN_READ_WRITE)
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
        let existing: bool = transaction
            .query_row(
                "SELECT EXISTS(SELECT 1 FROM app_settings WHERE key='recovery_activation')",
                [],
                |row| row.get(0),
            )
            .map_err(|_| JournalError::Io)?;
        if existing {
            return Err(JournalError::InvalidJournal);
        }
        let mut bytes = [0u8; 16];
        getrandom::fill(&mut bytes).map_err(|_| JournalError::Io)?;
        bytes[6] = (bytes[6] & 0x0f) | 0x40;
        bytes[8] = (bytes[8] & 0x3f) | 0x80;
        let generation: String = bytes.iter().map(|b| format!("{b:02x}")).collect();
        let value = serde_json::json!({"state":"preparing","generation":generation}).to_string();
        transaction
            .execute(
                "INSERT INTO app_settings(key,value) VALUES('recovery_activation',?)",
                [value],
            )
            .map_err(|_| JournalError::Io)?;
        transaction.commit().map_err(|_| JournalError::Io)?;
        Ok(RecoveryGeneration {
            guard: self,
            generation,
        })
    }
}
