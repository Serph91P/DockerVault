use super::{Control, Ftp, FtpError, SOCKET_TIMEOUT, expect, valid_argument};
use crate::ownership::{Work, WorkError};
use time::{Date, Month, PlainDateTime, Time, UtcDateTime, macros::format_description};
use tokio::{io::AsyncReadExt, time::timeout};

const MAX_LISTING_BYTES: usize = 16 * 1024 * 1024;

#[derive(Debug)]
pub struct FtpEntry {
    pub name: String,
    pub size: i64,
    pub is_dir: bool,
    pub modified: f64,
}

impl Ftp {
    pub fn list(&self, remote: String) -> Result<Work<Result<Vec<FtpEntry>, FtpError>>, WorkError> {
        let config = self.config.clone();
        self.owner.submit_async(move |cancel| async move {
            if !valid_argument(&remote)
                || (!remote.is_empty() && remote.split('/').any(|s| matches!(s, "" | "." | "..")))
            {
                return Err(FtpError::Config);
            }
            let remote = format!("{}/{}", config.base_path.trim_end_matches('/'), remote);
            if !valid_argument(&remote) {
                return Err(FtpError::Config);
            }
            let mut control = Control::connect(&config, &cancel).await?;
            let mut verb = "MLSD";
            let data = loop {
                let data = tokio::select! { biased;
                    _ = cancel.cancelled() => return Err(FtpError::Cancelled),
                    result = control.passive() => result?,
                };
                if cancel.is_cancelled() {
                    return Err(FtpError::Cancelled);
                }
                let status = control.command(verb, Some(&remote)).await?;
                if verb == "MLSD" && (500..510).contains(&status) {
                    // The first exchange never started transferring listing bytes.
                    drop(data);
                    verb = "LIST";
                    continue;
                }
                expect(status, &[125, 150])?;
                break data;
            };
            let mut data = data.take((MAX_LISTING_BYTES + 1) as u64);
            let mut listing = Vec::new();
            let body = tokio::select! { biased;
                _ = cancel.cancelled() => Err(FtpError::Cancelled),
                result = timeout(SOCKET_TIMEOUT, data.read_to_end(&mut listing)) => {
                    result.map_err(|_| FtpError::Timeout).and_then(|r| r.map_err(|_| FtpError::Io))
                        .and_then(|size| if size > MAX_LISTING_BYTES { Err(FtpError::Protocol) } else { Ok(()) })
                }
            };
            drop(data);
            if let Err(error) = body {
                control.abort_transfer().await?;
                return Err(error);
            }
            // The accepted aioftp server sends 200 after MLSD data EOF.
            expect(control.reply().await?, &[200, 226, 250])?;
            control.quit().await?;
            let listing = std::str::from_utf8(&listing).map_err(|_| FtpError::Protocol)?;
            let mut entries = Vec::new();
            for line in listing.split_inclusive('\n') {
                if cancel.is_cancelled() {
                    return Err(FtpError::Cancelled);
                }
                let entry = if verb == "MLSD" { mlsd(line)? } else { legacy(line)? };
                if let Some(entry) = entry {
                    entries.push(entry);
                }
            }
            if cancel.is_cancelled() { Err(FtpError::Cancelled) } else { Ok(entries) }
        })
    }
}

fn field<'a>(input: &mut &'a str) -> Result<&'a str, FtpError> {
    let (word, rest) = input
        .trim_start()
        .split_once(char::is_whitespace)
        .ok_or(FtpError::Protocol)?;
    *input = rest;
    Ok(word)
}

fn legacy(line: &str) -> Result<Option<FtpEntry>, FtpError> {
    let mut fields = line.strip_suffix("\r\n").ok_or(FtpError::Protocol)?;
    let first = field(&mut fields)?;
    let (size, mut is_dir, modified, mut name) = if first.contains('/') {
        let clock = field(&mut fields)?;
        let period = field(&mut fields)?;
        let time = PlainDateTime::parse(
            &format!("{first} {clock} {period}"),
            format_description!("[month]/[day]/[year] [hour repr:12]:[minute] [period]"),
        )
        .map_err(|_| FtpError::Protocol)?;
        let size = field(&mut fields)?;
        let is_dir = size == "<DIR>";
        let size = if is_dir {
            0
        } else {
            size.replace(',', "")
                .parse()
                .map_err(|_| FtpError::Protocol)?
        };
        (
            size,
            is_dir,
            time.as_utc().unix_timestamp(),
            fields.trim_start(),
        )
    } else {
        if first.len() != 10 || !matches!(first.as_bytes()[0], b'-' | b'd' | b'l') {
            return Err(FtpError::Protocol);
        }
        field(&mut fields)?
            .parse::<u64>()
            .map_err(|_| FtpError::Protocol)?;
        field(&mut fields)?; // owner
        field(&mut fields)?; // group
        let size = field(&mut fields)?
            .parse()
            .map_err(|_| FtpError::Protocol)?;
        let label = field(&mut fields)?;
        let month = [
            "Jan", "Feb", "Mar", "Apr", "May", "Jun", "Jul", "Aug", "Sep", "Oct", "Nov", "Dec",
        ]
        .iter()
        .position(|m| *m == label)
        .ok_or(FtpError::Protocol)?;
        let month = Month::try_from(month as u8 + 1).map_err(|_| FtpError::Protocol)?;
        let day = field(&mut fields)?
            .parse()
            .map_err(|_| FtpError::Protocol)?;
        let year_or_time = field(&mut fields)?;
        let modified = ls_date(month, day, year_or_time)?;
        (size, first.starts_with('d'), modified, fields.trim())
    };
    if first.starts_with('l') {
        let (source, target) = name.rsplit_once(" -> ").ok_or(FtpError::Protocol)?;
        name = source;
        is_dir = target.trim_end_matches(['\'', '"']).ends_with('/');
    }
    if matches!(name, "." | "..") {
        return Ok(None);
    }
    if !valid_argument(name) || name.is_empty() || name.contains('/') || size < 0 {
        return Err(FtpError::Protocol);
    }
    Ok(Some(FtpEntry {
        name: name.into(),
        size,
        is_dir,
        modified: modified as f64,
    }))
}

fn ls_date(month: Month, day: u8, year_or_time: &str) -> Result<i64, FtpError> {
    let date = |year, time| {
        Date::from_calendar_date(year, month, day)
            .map(|d| d.with_time(time).as_utc().unix_timestamp())
            .map_err(|_| FtpError::Protocol)
    };
    if !year_or_time.contains(':') {
        return date(
            year_or_time.parse().map_err(|_| FtpError::Protocol)?,
            Time::MIDNIGHT,
        );
    }
    let clock = Time::parse(year_or_time, format_description!("[hour]:[minute]"))
        .map_err(|_| FtpError::Protocol)?;
    let now = UtcDateTime::now();
    if month == Month::February && day == 29 {
        let (year, value) = (now.year() - 8..=now.year())
            .rev()
            .find_map(|y| date(y, clock).ok().map(|v| (y, v)))
            .ok_or(FtpError::Protocol)?;
        return Ok(if now.unix_timestamp() - value > 2 * 365 * 86400 {
            date(year + 4, clock).unwrap_or(value)
        } else {
            value
        });
    }
    let delta = now.unix_timestamp() - date(now.year(), clock)?;
    let adjust = if delta > 365 * 86400 / 2 {
        1
    } else if delta < -365 * 86400 / 2 {
        -1
    } else {
        0
    };
    date(now.year() + adjust, clock)
}

fn mlsd(line: &str) -> Result<Option<FtpEntry>, FtpError> {
    let line = line.strip_suffix("\r\n").ok_or(FtpError::Protocol)?;
    let (facts, name) = line.split_once(' ').ok_or(FtpError::Protocol)?;
    let mut size = 0;
    let mut modified = 0.0;
    let mut is_dir = false;
    for fact in facts.split_terminator(';') {
        let (key, value) = fact.split_once('=').ok_or(FtpError::Protocol)?;
        if key.eq_ignore_ascii_case("type") {
            if value.eq_ignore_ascii_case("cdir") || value.eq_ignore_ascii_case("pdir") {
                return Ok(None);
            }
            is_dir = value.eq_ignore_ascii_case("dir");
        } else if key.eq_ignore_ascii_case("size") {
            size = value.parse().unwrap_or(0);
        } else if key.eq_ignore_ascii_case("modify") {
            modified = value
                .get(..14)
                .and_then(|s| {
                    PlainDateTime::parse(
                        s,
                        format_description!("[year][month][day][hour][minute][second]"),
                    )
                    .ok()
                })
                .map_or(0.0, |date| date.as_utc().unix_timestamp() as f64);
        }
    }
    if !valid_argument(name) || matches!(name, "" | "." | "..") || name.contains('/') {
        return Err(FtpError::Protocol);
    }
    Ok(Some(FtpEntry {
        name: name.into(),
        size,
        is_dir,
        modified,
    }))
}
