//! Five-field APScheduler 3.11 preview semantics, with caller-owned TZif data.
//! No clock, filesystem, process, scheduler registration or ambient timezone access.
use crate::timestamp::Timestamp;
use std::collections::BTreeSet;
use time::{Date, Month, PlainDateTime, UtcDateTime};
pub use tz::TimeZone;
use tz::datetime::FoundDateTimeKind;
use unicode_general_category::{GeneralCategory, get_general_category};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CronError {
    Invalid,
    Expression(String),
    NoFuture,
    Cancelled,
}
impl std::fmt::Display for CronError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(match self {
            Self::Expression(detail) => detail,
            Self::NoFuture => "Cron expression has no future fire time",
            Self::Cancelled | Self::Invalid => "Invalid or unavailable schedule occurrence",
        })
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
impl FireTime {
    /// Python's aware datetime + timedelta adds in local wall time and resets fold.
    pub fn wall_end(self, seconds: i64, zone: &TimeZone) -> Result<Timestamp> {
        let date = self
            .timestamp
            .naive
            .checked_add(time::Duration::seconds(seconds))
            .filter(|date| (1..=9999).contains(&date.year()))
            .ok_or(CronError::NoFuture)?;
        let end = resolve(zone, date, false)?;
        Ok(Timestamp {
            naive: date,
            offset: Some(i64::from(end.offset) * 1_000_000),
        })
    }
}
#[derive(Clone, Copy)]
struct Wall {
    date: PlainDateTime,
    offset: i32,
    fold: bool,
}
impl Wall {
    fn epoch(self) -> i64 {
        (self.date.as_utc().unix_timestamp_nanos() / 1000) as i64
            - i64::from(self.offset) * 1_000_000
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
fn named_prefix(s: &str, names: &[&str], base: u32, unit: &str) -> Result<(u32, u32)> {
    let lookup = |name: &str| {
        names
            .iter()
            .position(|n| *n == name.to_lowercase())
            .map(|n| n as u32 + base)
            .ok_or_else(|| CronError::Expression(format!("Invalid {unit} name \"{name}\"")))
    };
    let first_end = s
        .char_indices()
        .find(|(_, c)| !letter(*c))
        .map_or(s.len(), |(i, _)| i);
    let first = lookup(&s[..first_end])?;
    let tail = &s[first_end..];
    let last = if let Some(tail) = tail.strip_prefix('-') {
        let end = tail
            .char_indices()
            .find(|(_, c)| !letter(*c))
            .map_or(tail.len(), |(i, _)| i);
        if end == 0 {
            first
        } else {
            lookup(&tail[..end])?
        }
    } else {
        first
    };
    if first > last {
        return Err(reversed_range());
    }
    Ok((first, last))
}
fn reversed_range() -> CronError {
    CronError::Expression("The minimum value in a range must not be higher than the maximum".into())
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
        Date::from_calendar_date(2024, Month::January, 1)
            .unwrap()
            .with_hms(0, 0, 0)
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
                    "month",
                )?
            } else {
                named_prefix(
                    item,
                    &["mon", "tue", "wed", "thu", "fri", "sat", "sun"],
                    0,
                    "weekday",
                )?
            };
            out.values.extend(first..=last);
            continue;
        }
        let name = [
            "year",
            "month",
            "day",
            "day_of_week",
            "hour",
            "minute",
            "second",
        ][index];
        let unrecognized = |_| {
            CronError::Expression(format!(
                "Unrecognized expression \"{item}\" for field \"{name}\""
            ))
        };
        let mut pieces = item.split('/');
        let range = pieces.next().unwrap();
        let step = pieces
            .next()
            .map(number)
            .transpose()
            .map_err(unrecognized)?;
        if pieces.next().is_some() {
            return Err(unrecognized(CronError::Invalid));
        }
        let (first, last, validation_last) = if range == "*" {
            (min, max, max)
        } else {
            let mut parts = range.split('-');
            let first = number(parts.next().unwrap()).map_err(unrecognized)?;
            let explicit = parts.next().map(number).transpose().map_err(unrecognized)?;
            if parts.next().is_some() {
                return Err(unrecognized(CronError::Invalid));
            }
            let last = explicit.unwrap_or(if step.is_some() { max } else { first });
            // APScheduler uses Python's `last or max`, including an explicit zero.
            (first, last, if last == 0 { max } else { last })
        };
        if step == Some(0) {
            return Err(CronError::Expression(
                "Increment must be higher than 0".into(),
            ));
        }
        if first < min
            || last > max
            || first > last
            || step.is_some_and(|s| s > validation_last.saturating_sub(first))
        {
            let detail = if first > last && (range.contains('-') || step.is_none()) {
                return Err(reversed_range());
            } else if let Some(s) = step.filter(|s| *s > max - min) {
                format!(
                    "the step value ({s}) is higher than the total range of the expression ({})",
                    max - min
                )
            } else if first < min {
                format!("the first value ({first}) is lower than the minimum value ({min})")
            } else if last > max {
                format!("the last value ({last}) is higher than the maximum value ({max})")
            } else if let Some(s) = step {
                format!(
                    "the step value ({s}) is higher than the total range of the expression ({})",
                    i64::from(validation_last) - i64::from(first)
                )
            } else {
                return Err(CronError::Invalid);
            };
            return Err(CronError::Expression(format!(
                "Error validating expression '{item}': {detail}"
            )));
        }
        out.values
            .extend((first..=last).step_by(step.unwrap_or(1) as usize));
    }
    if out.values.is_empty() && !out.last_day {
        return Err(CronError::Invalid);
    }
    Ok(out)
}
fn limits(index: usize, date: PlainDateTime) -> (u32, u32) {
    match index {
        0 => (1970, 9999),
        1 => (1, 12),
        2 => {
            let day = if date.month() == Month::December {
                31
            } else {
                Date::from_calendar_date(date.year(), date.month().next(), 1)
                    .unwrap()
                    .previous_day()
                    .unwrap()
                    .day()
            };
            (1, u32::from(day))
        }
        3 => (0, 6),
        4 => (0, 23),
        5 | 6 => (0, 59),
        _ => unreachable!(),
    }
}
fn value(index: usize, date: PlainDateTime) -> u32 {
    match index {
        0 => date.year() as u32,
        1 => date.month() as u32,
        2 => u32::from(date.day()),
        3 => u32::from(date.weekday().number_days_from_monday()),
        4 => u32::from(date.hour()),
        5 => u32::from(date.minute()),
        6 => u32::from(date.second()),
        _ => unreachable!(),
    }
}
fn make(values: &[u32; 7]) -> Result<PlainDateTime> {
    let month = Month::try_from(values[1] as u8).map_err(|_| CronError::NoFuture)?;
    Date::from_calendar_date(values[0] as i32, month, values[2] as u8)
        .and_then(|d| d.with_hms(values[4] as u8, values[5] as u8, values[6] as u8))
        .map_err(|_| CronError::NoFuture)
}
fn find(zone: &TimeZone, date: PlainDateTime) -> Result<Vec<FoundDateTimeKind>> {
    tz::DateTime::find(
        date.year(),
        date.month() as u8,
        date.day(),
        date.hour(),
        date.minute(),
        date.second(),
        date.nanosecond(),
        zone.as_ref(),
    )
    .map(|v| v.into_inner())
    .map_err(|_| CronError::Invalid)
}
fn resolve(zone: &TimeZone, date: PlainDateTime, fold: bool) -> Result<Wall> {
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
    let utc = UtcDateTime::from_unix_timestamp_nanos(i128::from(epoch) * 1000)
        .map_err(|_| CronError::NoFuture)?;
    if !(1..=9999).contains(&utc.year()) {
        return Err(CronError::NoFuture);
    }
    let d = tz::DateTime::from_timespec(
        epoch.div_euclid(1_000_000),
        (epoch.rem_euclid(1_000_000) * 1000) as u32,
        zone.as_ref(),
    )
    .map_err(|_| CronError::NoFuture)?;
    let date = Date::from_calendar_date(
        d.year(),
        Month::try_from(d.month()).map_err(|_| CronError::NoFuture)?,
        d.month_day(),
    )
    .and_then(|v| v.with_hms_micro(d.hour(), d.minute(), d.second(), d.nanoseconds() / 1000))
    .map_err(|_| CronError::NoFuture)?;
    if !(1..=9999).contains(&date.year()) {
        return Err(CronError::NoFuture);
    }
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
            return Err(CronError::Expression(format!(
                "Wrong number of fields; got {}, expected 5",
                words.len()
            )));
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
    fn next_value(&self, index: usize, date: PlainDateTime) -> Option<u32> {
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
        let delta = i64::try_from((make(&values)? - wall.date).whole_microseconds())
            .map_err(|_| CronError::NoFuture)?;
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
        self.next_from(base, zone, false, cancel)
    }

    // Continuation retains ZoneInfo identity; a fixed-offset input compares
    // differently during an ambiguous hour even if the instant is the same.
    pub fn next_after_fire(
        &self,
        base: FireTime,
        zone: &TimeZone,
        cancel: &dyn Fn() -> bool,
    ) -> Result<FireTime> {
        self.next_from(base.timestamp, zone, true, cancel)
    }

    fn next_from(
        &self,
        base: Timestamp,
        zone: &TimeZone,
        same_zone: bool,
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
            Some(offset) => (base.naive.as_utc().unix_timestamp_nanos() / 1000) as i64 - offset,
            None => resolve(zone, base.naive, false)?.epoch(),
        };
        let mut wall = project(zone, epoch)?;
        // Python inter-zone equality is false in a fold; naive bases share ZoneInfo.
        let equal = if base.offset.is_none() || same_zone {
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
