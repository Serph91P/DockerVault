//! Preserve SQLAlchemy's six-digit wall-time storage and Python ISO API strings.
use chrono::{DateTime, Datelike, FixedOffset, NaiveDate, NaiveDateTime, Timelike};

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
    offset: Option<FixedOffset>,
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
        let value = value.replacen(' ', "T", 1);
        let (naive, offset) = if let Ok(value) = DateTime::parse_from_rfc3339(&value) {
            (value.naive_local(), Some(*value.offset()))
        } else if let Ok(value) = NaiveDateTime::parse_from_str(&value, "%Y-%m-%dT%H:%M:%S%.f") {
            (value, None)
        } else if let Ok(value) = NaiveDate::parse_from_str(&value, "%Y-%m-%d") {
            (value.and_hms_opt(0, 0, 0).ok_or(InvalidTimestamp)?, None)
        } else {
            return Err(InvalidTimestamp);
        };
        if !(1..=9999).contains(&naive.year()) || naive.nanosecond() >= 1_000_000_000 {
            return Err(InvalidTimestamp);
        }
        // Python datetime truncates sub-microsecond input instead of rounding it.
        let naive = naive
            .with_nanosecond(naive.nanosecond() / 1000 * 1000)
            .ok_or(InvalidTimestamp)?;
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
            text.push_str(&offset.to_string());
        }
        text
    }
}
