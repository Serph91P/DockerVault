//! Five-field APScheduler 3.11 preview semantics, with caller-owned TZif data.
//! No clock, filesystem, process, scheduler registration or ambient timezone access.
use crate::timestamp::Timestamp;
use chrono::{Datelike, NaiveDate, NaiveDateTime, Timelike};
use std::collections::BTreeSet;
pub use tz::TimeZone;
use tz::datetime::FoundDateTimeKind;
use unicode_general_category::{GeneralCategory, get_general_category};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CronError {
    Invalid,
    NoFuture,
    Cancelled,
}
impl std::fmt::Display for CronError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Invalid or unavailable schedule occurrence")
    }
}
impl std::error::Error for CronError {}
type Result<T> = std::result::Result<T, CronError>;
#[derive(Clone)]
struct Field {
    values: BTreeSet<u32>,
    last_day: bool,
}
#[derive(Clone)]
pub struct Schedule {
    fields: Vec<Field>,
}
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FireTime {
    pub timestamp: Timestamp,
    pub fold: bool,
}
#[derive(Clone, Copy)]
struct Wall {
    date: NaiveDateTime,
    offset: i32,
    fold: bool,
}
impl Wall {
    fn epoch(self) -> i64 {
        self.date.and_utc().timestamp_micros() - i64::from(self.offset) * 1_000_000
    }
}
fn whitespace(c: char) -> bool {
    c.is_whitespace() || matches!(c, '\u{1c}'..='\u{1f}')
}
fn digit(c: char) -> Option<u32> {
    if c.is_ascii_digit() {
        return c.to_digit(10);
    }
    if get_general_category(c) != GeneralCategory::DecimalNumber {
        return None;
    }
    let mut start = u32::from(c);
    while start > 0
        && char::from_u32(start - 1)
            .is_some_and(|p| get_general_category(p) == GeneralCategory::DecimalNumber)
    {
        start -= 1;
    }
    Some((u32::from(c) - start) % 10)
}
fn number(s: &str) -> Result<u32> {
    if s.is_empty() {
        return Err(CronError::Invalid);
    }
    s.chars().try_fold(0_u32, |v, c| {
        v.checked_mul(10)
            .and_then(|n| n.checked_add(digit(c)?))
            .ok_or(CronError::Invalid)
    })
}
fn letter(c: char) -> bool {
    c.is_ascii_alphabetic() || matches!(c, 'İ' | 'ı' | 'ſ' | 'K')
}
fn named_prefix(s: &str, names: &[&str], base: u32) -> Result<(u32, u32)> {
    let first_end = s
        .char_indices()
        .find(|(_, c)| !letter(*c))
        .map_or(s.len(), |(i, _)| i);
    let first = names
        .iter()
        .position(|n| *n == s[..first_end].to_lowercase())
        .ok_or(CronError::Invalid)? as u32
        + base;
    let tail = &s[first_end..];
    let last = if let Some(tail) = tail.strip_prefix('-') {
        let end = tail
            .char_indices()
            .find(|(_, c)| !letter(*c))
            .map_or(tail.len(), |(i, _)| i);
        if end == 0 {
            first
        } else {
            names
                .iter()
                .position(|n| *n == tail[..end].to_lowercase())
                .ok_or(CronError::Invalid)? as u32
                + base
        }
    } else {
        first
    };
    if first > last {
        return Err(CronError::Invalid);
    }
    Ok((first, last))
}
fn field(text: &str, index: usize) -> Result<Field> {
    // The implicit year wildcard needs no eight-thousand-element allocation.
    if index == 0 {
        return Ok(Field {
            values: BTreeSet::new(),
            last_day: false,
        });
    }
    let (min, max) = limits(
        index,
        NaiveDate::from_ymd_opt(2024, 1, 1)
            .unwrap()
            .and_hms_opt(0, 0, 0)
            .unwrap(),
    );
    let mut out = Field {
        values: BTreeSet::new(),
        last_day: false,
    };
    for item in text.split(',') {
        let keyword: String = item
            .chars()
            .take(4)
            .map(|c| {
                if c == 'ſ' {
                    's'
                } else {
                    c.to_ascii_lowercase()
                }
            })
            .collect();
        if index == 2 && keyword == "last" {
            out.last_day = true;
            continue;
        }
        if (index == 1 || index == 3) && item.chars().next().is_some_and(letter) {
            let (first, last) = if index == 1 {
                named_prefix(
                    item,
                    &[
                        "jan", "feb", "mar", "apr", "may", "jun", "jul", "aug", "sep", "oct",
                        "nov", "dec",
                    ],
                    1,
                )?
            } else {
                named_prefix(item, &["mon", "tue", "wed", "thu", "fri", "sat", "sun"], 0)?
            };
            out.values.extend(first..=last);
            continue;
        }
        let mut pieces = item.split('/');
        let range = pieces.next().unwrap();
        let step = pieces.next().map(number).transpose()?;
        if pieces.next().is_some() || step == Some(0) {
            return Err(CronError::Invalid);
        }
        let (first, last, validation_last) = if range == "*" {
            (min, max, max)
        } else {
            let mut parts = range.split('-');
            let first = number(parts.next().unwrap())?;
            let explicit = parts.next().map(number).transpose()?;
            if parts.next().is_some() {
                return Err(CronError::Invalid);
            }
            let last = explicit.unwrap_or(if step.is_some() { max } else { first });
            // APScheduler uses Python's `last or max`, including an explicit zero.
            (first, last, if last == 0 { max } else { last })
        };
        if first < min
            || last > max
            || first > last
            || step.is_some_and(|s| s > validation_last.saturating_sub(first))
        {
            return Err(CronError::Invalid);
        }
        out.values
            .extend((first..=last).step_by(step.unwrap_or(1) as usize));
    }
    if out.values.is_empty() && !out.last_day {
        return Err(CronError::Invalid);
    }
    Ok(out)
}
fn limits(index: usize, date: NaiveDateTime) -> (u32, u32) {
    match index {
        0 => (1970, 9999),
        1 => (1, 12),
        2 => {
            let day = if date.month() == 12 {
                31
            } else {
                NaiveDate::from_ymd_opt(date.year(), date.month() + 1, 1)
                    .unwrap()
                    .pred_opt()
                    .unwrap()
                    .day()
            };
            (1, day)
        }
        3 => (0, 6),
        4 => (0, 23),
        5 | 6 => (0, 59),
        _ => unreachable!(),
    }
}
fn value(index: usize, date: NaiveDateTime) -> u32 {
    match index {
        0 => date.year() as u32,
        1 => date.month(),
        2 => date.day(),
        3 => date.weekday().num_days_from_monday(),
        4 => date.hour(),
        5 => date.minute(),
        6 => date.second(),
        _ => unreachable!(),
    }
}
fn make(values: &[u32; 7]) -> Result<NaiveDateTime> {
    NaiveDate::from_ymd_opt(values[0] as i32, values[1], values[2])
        .and_then(|d| d.and_hms_opt(values[4], values[5], values[6]))
        .ok_or(CronError::NoFuture)
}
fn find(zone: &TimeZone, date: NaiveDateTime) -> Result<Vec<FoundDateTimeKind>> {
    tz::DateTime::find(
        date.year(),
        date.month() as u8,
        date.day() as u8,
        date.hour() as u8,
        date.minute() as u8,
        date.second() as u8,
        date.nanosecond(),
        zone.as_ref(),
    )
    .map(|v| v.into_inner())
    .map_err(|_| CronError::Invalid)
}
fn resolve(zone: &TimeZone, date: NaiveDateTime, fold: bool) -> Result<Wall> {
    let found = find(zone, date)?;
    let selected = if fold { found.last() } else { found.first() }.ok_or(CronError::Invalid)?;
    let offset = match selected {
        FoundDateTimeKind::Normal(d) => d.local_time_type().ut_offset(),
        FoundDateTimeKind::Skipped {
            before_transition,
            after_transition,
        } => if fold {
            after_transition
        } else {
            before_transition
        }
        .local_time_type()
        .ut_offset(),
    };
    Ok(Wall { date, offset, fold })
}
fn project(zone: &TimeZone, epoch: i64) -> Result<Wall> {
    // datetime_utc_add must first represent the UTC intermediate in Python's range.
    chrono::DateTime::from_timestamp_micros(epoch)
        .filter(|v| (1..=9999).contains(&v.year()))
        .ok_or(CronError::NoFuture)?;
    let d = tz::DateTime::from_timespec(
        epoch.div_euclid(1_000_000),
        (epoch.rem_euclid(1_000_000) * 1000) as u32,
        zone.as_ref(),
    )
    .map_err(|_| CronError::NoFuture)?;
    let date = NaiveDate::from_ymd_opt(d.year(), u32::from(d.month()), u32::from(d.month_day()))
        .and_then(|v| {
            v.and_hms_micro_opt(
                u32::from(d.hour()),
                u32::from(d.minute()),
                u32::from(d.second()),
                d.nanoseconds() / 1000,
            )
        })
        .filter(|v| (1..=9999).contains(&v.year()))
        .ok_or(CronError::NoFuture)?;
    let found = find(zone, date)?;
    let fold = found.len() > 1
        && matches!(found.last(),Some(FoundDateTimeKind::Normal(last)) if last.unix_time()==d.unix_time());
    Ok(Wall {
        date,
        offset: d.local_time_type().ut_offset(),
        fold,
    })
}
impl Schedule {
    pub fn parse(expression: &str) -> Result<Self> {
        let words: Vec<_> = expression
            .split(whitespace)
            .filter(|s| !s.is_empty())
            .collect();
        if words.len() != 5 {
            return Err(CronError::Invalid);
        }
        let mut fields = Vec::new();
        for (index, text) in [
            (0, "*"),
            (1, words[3]),
            (2, words[2]),
            (3, words[4]),
            (4, words[1]),
            (5, words[0]),
            (6, "0"),
        ] {
            fields.push(field(text, index)?);
        }
        Ok(Self { fields })
    }
    fn next_value(&self, index: usize, date: NaiveDateTime) -> Option<u32> {
        let current = value(index, date);
        let max = limits(index, date).1;
        if index == 0 {
            return Some(current.max(1970));
        }
        let ordinary = self.fields[index]
            .values
            .range(current..=max)
            .next()
            .copied();
        if self.fields[index].last_day {
            Some(ordinary.map_or(max, |n| n.min(max)))
        } else {
            ordinary
        }
    }
    fn increment(&self, zone: &TimeZone, wall: Wall, mut index: i32) -> Result<(Wall, i32)> {
        let mut values = [0; 7];
        let mut i = 0_i32;
        while i < 7 {
            if index < 0 {
                return Err(CronError::NoFuture);
            }
            if i == 3 {
                if index == i {
                    index -= 1;
                    i -= 1;
                } else {
                    i += 1;
                }
                continue;
            }
            let slot = i as usize;
            if i > index {
                values[slot] = limits(slot, wall.date).0;
            } else if i < index {
                values[slot] = value(slot, wall.date);
            } else if value(slot, wall.date) == limits(slot, wall.date).1 {
                index -= 1;
                i -= 1;
                continue;
            } else {
                values[slot] = value(slot, wall.date) + 1;
            }
            i += 1;
        }
        let delta = make(&values)?
            .signed_duration_since(wall.date)
            .num_microseconds()
            .ok_or(CronError::NoFuture)?;
        Ok((
            project(
                zone,
                wall.epoch().checked_add(delta).ok_or(CronError::NoFuture)?,
            )?,
            index,
        ))
    }
    /// Match published `next_run(expression, base)`, not a Unix-cron approximation.
    pub fn next(
        &self,
        base: Timestamp,
        zone: &TimeZone,
        cancel: &dyn Fn() -> bool,
    ) -> Result<FireTime> {
        if cancel() {
            return Err(CronError::Cancelled);
        }
        let normalized;
        // Python ZoneInfo deliberately ignores TZif leap-second records.
        let zone = if zone.as_ref().leap_seconds().is_empty() {
            zone
        } else {
            let z = zone.as_ref();
            normalized = TimeZone::new(
                z.transitions().to_vec(),
                z.local_time_types().to_vec(),
                Vec::new(),
                *z.extra_rule(),
            )
            .map_err(|_| CronError::Invalid)?;
            &normalized
        };
        let epoch = match base.offset {
            Some(offset) => base.naive.and_utc().timestamp_micros() - offset,
            None => resolve(zone, base.naive, false)?.epoch(),
        };
        let mut wall = project(zone, epoch)?;
        // Python inter-zone equality is false in a fold; naive bases share ZoneInfo.
        let equal = if base.offset.is_none() {
            wall.date == base.naive
        } else {
            resolve(zone, wall.date, false)?.offset == resolve(zone, wall.date, true)?.offset
        };
        if equal {
            wall = project(zone, epoch.checked_add(1).ok_or(CronError::NoFuture)?)?;
        }
        if wall.date.nanosecond() != 0 {
            wall = project(
                zone,
                wall.epoch() + i64::from(1_000_000 - wall.date.nanosecond() / 1000),
            )?;
        }
        let mut index = 0_i32;
        while index < 7 {
            if cancel() {
                return Err(CronError::Cancelled);
            }
            let i = index as usize;
            let current = value(i, wall.date);
            match self.next_value(i, wall.date) {
                None => {
                    (wall, index) = self.increment(zone, wall, index - 1)?;
                }
                Some(next) if next > current => {
                    if i == 3 {
                        (wall, index) = self.increment(zone, wall, index)?;
                    } else {
                        let mut values = [0; 7];
                        for (j, v) in values.iter_mut().enumerate() {
                            if j != 3 {
                                *v = if j < i {
                                    value(j, wall.date)
                                } else if j == i {
                                    next
                                } else {
                                    limits(j, wall.date).0
                                };
                            }
                        }
                        wall = resolve(zone, make(&values)?, wall.fold)?;
                        index += 1;
                    }
                }
                Some(_) => index += 1,
            }
        }
        Ok(FireTime {
            timestamp: Timestamp {
                naive: wall.date,
                offset: Some(i64::from(wall.offset) * 1_000_000),
            },
            fold: wall.fold,
        })
    }
}
