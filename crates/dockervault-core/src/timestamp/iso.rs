//! CPython 3.14 ISO framing around Chrono's validated Gregorian calendar.
//! See the CPython attribution in RUST_DEPENDENCY_NOTICES.md.
use super::InvalidTimestamp;
use chrono::{Datelike, NaiveDate, NaiveDateTime, Weekday};

fn byte(input: &[u8], at: usize) -> u8 {
    input.get(at).copied().unwrap_or(0)
}
fn digits(input: &[u8], at: usize, count: usize) -> Result<u32, InvalidTimestamp> {
    input
        .get(at..at + count)
        .ok_or(InvalidTimestamp)?
        .iter()
        .try_fold(0, |n, b| {
            if b.is_ascii_digit() {
                Ok(n * 10 + u32::from(b - b'0'))
            } else {
                Err(InvalidTimestamp)
            }
        })
}
fn separator(input: &[u8]) -> Result<usize, InvalidTimestamp> {
    let len = input.len();
    if len < 7 {
        return Err(InvalidTimestamp);
    }
    if len == 7 {
        return Ok(7);
    }
    if byte(input, 4) == b'-' {
        if byte(input, 5) != b'W' {
            return Ok(10);
        }
        if len < 8 {
            return Err(InvalidTimestamp);
        }
        if len > 8 && byte(input, 8) == b'-' {
            if len == 9 {
                return Err(InvalidTimestamp);
            }
            return Ok(if len > 10 && byte(input, 10).is_ascii_digit() {
                8
            } else {
                10
            });
        }
        Ok(8)
    } else if byte(input, 4) == b'W' {
        let mut index = 7;
        while index < len && byte(input, index).is_ascii_digit() {
            index += 1;
        }
        Ok(if index < 9 {
            index
        } else if index % 2 == 0 {
            7
        } else {
            8
        })
    } else {
        Ok(8)
    }
}
fn date(input: &[u8]) -> Result<(i32, u32, u32), InvalidTimestamp> {
    let year = digits(input, 0, 4)? as i32;
    let separated = byte(input, 4) == b'-';
    let start = if separated { 5 } else { 4 };
    if byte(input, start) == b'W' {
        if !(1..=9999).contains(&year) {
            return Err(InvalidTimestamp);
        }
        let week = digits(input, start + 1, 2)?;
        let end = start + 3;
        let day = if input.len() == end {
            1
        } else {
            let pos = if separated {
                if byte(input, end) != b'-' {
                    return Err(InvalidTimestamp);
                }
                end + 1
            } else {
                end
            };
            if input.len() != pos + 1 {
                return Err(InvalidTimestamp);
            }
            digits(input, pos, 1)?
        };
        let day = match day {
            1 => Weekday::Mon,
            2 => Weekday::Tue,
            3 => Weekday::Wed,
            4 => Weekday::Thu,
            5 => Weekday::Fri,
            6 => Weekday::Sat,
            7 => Weekday::Sun,
            _ => return Err(InvalidTimestamp),
        };
        let date = NaiveDate::from_isoywd_opt(year, week, day).ok_or(InvalidTimestamp)?;
        Ok((date.year(), date.month(), date.day()))
    } else {
        if input.len() != if separated { 10 } else { 8 } {
            return Err(InvalidTimestamp);
        }
        let month = digits(input, start, 2)?;
        let day_at = if separated {
            if byte(input, start + 2) != b'-' {
                return Err(InvalidTimestamp);
            }
            start + 3
        } else {
            start + 2
        };
        Ok((year, month, digits(input, day_at, 2)?))
    }
}
// Return the source parser's trailing-character state: timezone framing consumes it.
fn clock(input: &[u8], end: usize) -> Result<([u32; 4], bool), InvalidTimestamp> {
    let mut fields = [0; 4];
    let mut pos = 0;
    let mut separated = true;
    for index in 0..3 {
        fields[index] = digits(input, pos, 2)?;
        pos += 2;
        let c = byte(input, pos);
        pos += 1;
        if index == 0 {
            separated = c == b':';
        }
        if c == b'.' || c == b',' {
            if index < 2 || pos >= end {
                return Err(InvalidTimestamp);
            }
            break;
        }
        if pos >= end {
            return Ok((fields, c != 0));
        }
        if separated && c == b':' {
            if index == 2 {
                return Err(InvalidTimestamp);
            }
        } else if !separated {
            pos -= 1;
        } else {
            return Err(InvalidTimestamp);
        }
    }
    let length = end.checked_sub(pos).ok_or(InvalidTimestamp)?.min(6);
    if length == 0 {
        return Err(InvalidTimestamp);
    }
    fields[3] = digits(input, pos, length)? * 10_u32.pow(6 - length as u32);
    pos += length;
    while byte(input, pos).is_ascii_digit() {
        pos += 1;
    }
    Ok((fields, byte(input, pos) != 0))
}
pub(super) fn parse(input: &str) -> Result<(NaiveDateTime, Option<i64>), InvalidTimestamp> {
    let bytes = input.as_bytes();
    let split = separator(bytes)?;
    let (mut year, mut month, mut day) = date(bytes.get(..split).ok_or(InvalidTimestamp)?)?;
    let (fields, offset) = if bytes.len() == split {
        ([0; 4], None)
    } else {
        let tail = input.get(split..).ok_or(InvalidTimestamp)?;
        let width = tail.chars().next().ok_or(InvalidTimestamp)?.len_utf8();
        let time = tail.as_bytes().get(width..).ok_or(InvalidTimestamp)?;
        let tz = time
            .iter()
            .position(|b| matches!(*b, b'Z' | b'+' | b'-'))
            .unwrap_or(time.len());
        let (fields, trailing) = clock(time, tz)?;
        let offset = if tz == time.len() {
            if trailing {
                return Err(InvalidTimestamp);
            }
            None
        } else if time[tz] == b'Z' {
            if byte(time, tz + 1) != 0 {
                return Err(InvalidTimestamp);
            }
            Some(0)
        } else {
            let (zone, trailing) = clock(&time[tz + 1..], time.len() - tz - 1)?;
            if trailing {
                return Err(InvalidTimestamp);
            }
            let micros = (i64::from(zone[0]) * 3600 + i64::from(zone[1]) * 60 + i64::from(zone[2]))
                * 1_000_000
                + i64::from(zone[3]);
            if micros >= 86_400_000_000 {
                return Err(InvalidTimestamp);
            }
            Some(if time[tz] == b'-' { -micros } else { micros })
        };
        (fields, offset)
    };
    let [mut hour, minute, second, micros] = fields;
    if hour == 24 && (1..=12).contains(&month) {
        let last = match month {
            2 => {
                if year % 4 == 0 && (year % 100 != 0 || year % 400 == 0) {
                    29
                } else {
                    28
                }
            }
            4 | 6 | 9 | 11 => 30,
            _ => 31,
        };
        if day <= last {
            if minute != 0 || second != 0 || micros != 0 {
                return Err(InvalidTimestamp);
            }
            hour = 0;
            day += 1;
            if day > last {
                day = 1;
                month += 1;
                if month > 12 {
                    month = 1;
                    year += 1;
                }
            }
        }
    }
    if !(1..=9999).contains(&year) {
        return Err(InvalidTimestamp);
    }
    let naive = NaiveDate::from_ymd_opt(year, month, day)
        .and_then(|d| d.and_hms_micro_opt(hour, minute, second, micros))
        .ok_or(InvalidTimestamp)?;
    Ok((naive, offset))
}
