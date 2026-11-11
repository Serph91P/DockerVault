//! Preserve SQLAlchemy's six-digit wall-time storage and Python ISO API strings.
mod iso;
use chrono::{NaiveDateTime, Timelike};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidTimestamp;
impl std::fmt::Display for InvalidTimestamp {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Invalid persisted timestamp")
    }
}
impl std::error::Error for InvalidTimestamp {}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Timestamp {
    naive: NaiveDateTime,
    offset: Option<i64>,
}
impl serde::Serialize for Timestamp {
    fn serialize<S: serde::Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        serializer.serialize_str(&self.api())
    }
}
impl<'de> serde::Deserialize<'de> for Timestamp {
    fn deserialize<D: serde::Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let value = <String as serde::Deserialize>::deserialize(deserializer)?;
        Self::parse(&value).map_err(|_| serde::de::Error::custom("Invalid persisted timestamp"))
    }
}
impl Timestamp {
    pub fn parse(value: &str) -> Result<Self, InvalidTimestamp> {
        let (naive, offset) = iso::parse(value)?;
        Ok(Self { naive, offset })
    }
    pub fn sql(self) -> String {
        self.naive.format("%Y-%m-%d %H:%M:%S%.6f").to_string()
    }
    pub fn api(self) -> String {
        let mut text = self
            .naive
            .format(if self.naive.nanosecond() == 0 {
                "%Y-%m-%dT%H:%M:%S"
            } else {
                "%Y-%m-%dT%H:%M:%S%.6f"
            })
            .to_string();
        if let Some(offset) = self.offset {
            let sign = if offset < 0 { '-' } else { '+' };
            let micros = offset.unsigned_abs();
            let seconds = micros / 1_000_000;
            text.push_str(&format!(
                "{sign}{:02}:{:02}",
                seconds / 3600,
                seconds / 60 % 60
            ));
            if seconds % 60 != 0 || micros % 1_000_000 != 0 {
                text.push_str(&format!(":{:02}", seconds % 60));
                if micros % 1_000_000 != 0 {
                    text.push_str(&format!(".{:06}", micros % 1_000_000));
                }
            }
        }
        text
    }
}
