//! Preserve SQLAlchemy's six-digit wall-time storage and Python ISO API strings.
mod iso;
use time::PlainDateTime;

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
    pub(crate) naive: PlainDateTime,
    pub(crate) offset: Option<i64>,
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
    pub fn unix_micros(self) -> Option<i64> {
        let offset = self.offset?;
        i64::try_from(self.naive.as_utc().unix_timestamp_nanos() / 1000)
            .ok()?
            .checked_sub(offset)
    }
    pub fn parse(value: &str) -> Result<Self, InvalidTimestamp> {
        let (naive, offset) = iso::parse(value)?;
        Ok(Self { naive, offset })
    }
    pub fn sql(self) -> String {
        let d = self.naive;
        format!(
            "{:04}-{:02}-{:02} {:02}:{:02}:{:02}.{:06}",
            d.year(),
            d.month() as u8,
            d.day(),
            d.hour(),
            d.minute(),
            d.second(),
            d.microsecond()
        )
    }
    pub fn api(self) -> String {
        let mut text = self.sql().replace(' ', "T");
        if self.naive.nanosecond() == 0 {
            text.truncate(19);
        }
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
