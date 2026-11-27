use axum::{
    body::{Body, Bytes},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use futures_util::stream;
use md5::{Digest, Md5};
use std::{collections::VecDeque, fs::File, io, os::unix::fs::MetadataExt};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

const CHUNK: usize = 64 * 1024;

pub(in crate::http) struct Validators {
    modified: String,
    etag: String,
}
impl Validators {
    pub(in crate::http) fn read(file: &File, length: u64) -> io::Result<Self> {
        let stat = file.metadata()?;
        let modified = time::OffsetDateTime::from_unix_timestamp(stat.mtime())
            .map_err(io::Error::other)?
            .format(&time::format_description::well_known::Rfc2822)
            .map_err(io::Error::other)?
            .replace("+0000", "GMT");
        let stamp =
            (i128::from(stat.mtime()) * 1_000_000_000 + i128::from(stat.mtime_nsec())) as f64 / 1e9;
        // Match Starlette's metadata validator; this is not a security digest.
        let tag = Md5::digest(format!("{stamp:?}-{length}"));
        let tag: String = tag.iter().map(|byte| format!("{byte:02x}")).collect();
        Ok(Self {
            modified,
            etag: format!("\"{tag}\""),
        })
    }
}

enum Part {
    Text(Bytes),
    File { start: u64, end: u64 },
}

pub(in crate::http) fn response(
    file: File,
    length: u64,
    slot: impl Send + 'static,
    disposition: String,
    validators: Validators,
    request: &HeaderMap,
    media: &'static str,
) -> Response {
    let if_range = request.get(header::IF_RANGE);
    let range = request.get(header::RANGE).filter(|_| {
        if_range.is_none_or(|value| {
            value.as_bytes() == validators.modified.as_bytes()
                || value.as_bytes() == validators.etag.as_bytes()
        })
    });
    let ranges = match range {
        Some(value) => match parse(value.to_str().unwrap_or_default(), length) {
            Ok(ranges) => ranges,
            Err(RangeError::Bad(message)) => {
                return (StatusCode::BAD_REQUEST, message).into_response();
            }
            Err(RangeError::Unsatisfiable) => {
                return (
                    StatusCode::RANGE_NOT_SATISFIABLE,
                    [(header::CONTENT_RANGE, format!("bytes */{length}"))],
                    "",
                )
                    .into_response();
            }
        },
        None => Vec::new(),
    };
    let mut parts = VecDeque::new();
    let mut content_range = None;
    let mut content_type = media.to_owned();
    let (status, size) = match ranges.as_slice() {
        [] => {
            parts.push_back(Part::File {
                start: 0,
                end: length,
            });
            (StatusCode::OK, length)
        }
        &[(start, end)] => {
            content_range = Some(format!("bytes {start}-{}/{length}", end - 1));
            parts.push_back(Part::File { start, end });
            (StatusCode::PARTIAL_CONTENT, end - start)
        }
        _ => {
            let mut random = [0; 13];
            if getrandom::fill(&mut random).is_err() {
                return super::storage_error(StatusCode::INTERNAL_SERVER_ERROR, "Download failed");
            }
            let boundary: String = random.iter().map(|byte| format!("{byte:02x}")).collect();
            content_type = format!("multipart/byteranges; boundary={boundary}");
            let footer = format!("--{boundary}--");
            let mut size = footer.len() as u64;
            for (start, end) in ranges {
                let prefix = format!(
                    "--{boundary}\r\nContent-Type: {media}\r\nContent-Range: bytes {start}-{}/{length}\r\n\r\n",
                    end - 1
                );
                size += prefix.len() as u64 + (end - start) + 2;
                parts.push_back(Part::Text(prefix.into()));
                parts.push_back(Part::File { start, end });
                parts.push_back(Part::Text(Bytes::from_static(b"\r\n")));
            }
            parts.push_back(Part::Text(footer.into()));
            (StatusCode::PARTIAL_CONTENT, size)
        }
    };
    let body = Body::from_stream(stream::try_unfold(
        (tokio::fs::File::from_std(file), parts, 0, slot),
        |(mut file, mut parts, mut remaining, slot)| async move {
            if remaining == 0 {
                match parts.pop_front() {
                    Some(Part::Text(bytes)) => {
                        return Ok(Some((bytes, (file, parts, remaining, slot))));
                    }
                    Some(Part::File { start, end }) if start != end => {
                        if start != 0 {
                            file.seek(io::SeekFrom::Start(start)).await?;
                        }
                        remaining = end - start;
                    }
                    _ => return Ok(None),
                }
            }
            let mut bytes = vec![0; remaining.min(CHUNK as u64) as usize];
            let count = file.read(&mut bytes).await?;
            if count == 0 {
                return Err(io::Error::new(
                    io::ErrorKind::UnexpectedEof,
                    "Snapshot ended early",
                ));
            }
            remaining -= count as u64;
            bytes.truncate(count);
            Ok(Some((Bytes::from(bytes), (file, parts, remaining, slot))))
        },
    ));
    let mut response = (
        status,
        [
            (header::CONTENT_TYPE, content_type),
            (header::CONTENT_DISPOSITION, disposition),
            (header::CONTENT_LENGTH, size.to_string()),
            (header::ACCEPT_RANGES, "bytes".to_owned()),
            (header::LAST_MODIFIED, validators.modified),
            (header::ETAG, validators.etag),
        ],
        body,
    )
        .into_response();
    if let Some(content_range) = content_range {
        response.headers_mut().insert(
            header::CONTENT_RANGE,
            content_range.parse().expect("numeric range"),
        );
    }
    response
}

enum RangeError {
    Bad(&'static str),
    Unsatisfiable,
}
fn parse(value: &str, size: u64) -> Result<Vec<(u64, u64)>, RangeError> {
    let (unit, value) = value
        .split_once('=')
        .ok_or(RangeError::Bad("Malformed range header."))?;
    if !unit.trim().eq_ignore_ascii_case("bytes") {
        return Err(RangeError::Bad("Only support bytes range"));
    }
    if value.split(',').count() > 100 {
        return Ok(Vec::new());
    }
    let size = i128::from(size);
    let mut ranges: Vec<_> = value
        .split(',')
        .filter_map(|part| {
            let (start, end) = part.split_once('-')?;
            let (start, end) = (start.trim(), end.trim());
            Some(if start.is_empty() {
                (size.saturating_sub(end.parse::<i128>().ok()?).max(0), size)
            } else {
                (
                    start.parse().ok()?,
                    if end.is_empty() {
                        size
                    } else {
                        end.parse::<i128>().ok()?.saturating_add(1).min(size)
                    },
                )
            })
        })
        .collect();
    if ranges.is_empty() {
        return Err(RangeError::Bad("Range header: range must be requested"));
    }
    if ranges.iter().any(|&(start, _)| start < 0 || start >= size) {
        return Err(RangeError::Unsatisfiable);
    }
    if ranges.iter().any(|&(start, end)| start >= end) {
        return Err(RangeError::Bad("Range header: start must be less than end"));
    }
    ranges.sort_unstable();
    let mut merged: Vec<(u64, u64)> = Vec::new();
    for (start, end) in ranges {
        let (start, end) = (start as u64, end as u64);
        if let Some(last) = merged.last_mut().filter(|last| start <= last.1) {
            last.1 = last.1.max(end);
        } else {
            merged.push((start, end));
        }
    }
    Ok(merged)
}
