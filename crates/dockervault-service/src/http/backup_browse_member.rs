use super::{
    HttpState, INTEGER, detail, internal, query_integer, required_user, temp, unreadable,
    validation, with_local_archive,
};
use crate::{
    http::storage::download::{Validators, response},
    restore::{ArchiveReader, Member, MemberKind},
};
use axum::{
    extract::{Path, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::Response,
};
use serde_json::json;
use sha2::{Digest, Sha256};
use std::{
    collections::HashSet,
    fs::File,
    io::{self, Read, Seek, SeekFrom, Write},
    sync::Arc,
};

pub(in crate::http) async fn member(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path((id, name)): Path<(String, String)>,
) -> Response {
    if let Err(error) = required_user(&state, &headers).await {
        return *error;
    }
    let Some(id) = query_integer(&id) else {
        return *validation(vec![json!({"field":"path.backup_id","message":INTEGER})]);
    };
    let normalized = normalize(name.as_bytes());
    let first = normalized
        .iter()
        .position(|c| !matches!(c, b'/' | b'\\'))
        .unwrap_or(normalized.len());
    let normalized = normalized[first..].to_vec();
    if normalized.split(|&c| c == b'/').any(|c| c == b"..") {
        return detail(StatusCode::BAD_REQUEST, "Invalid file path");
    }
    let current = sqlx::query_as::<_, (Option<String>, Option<String>, Option<bool>)>(
        "SELECT status,file_path,encrypted FROM backups WHERE id=?",
    )
    .bind(id)
    .fetch_optional(state.auth.pool())
    .await;
    let path = match current {
        Ok(None) => return detail(StatusCode::NOT_FOUND, "Backup not found"),
        Ok(Some((status, _, _))) if status.as_deref() != Some("COMPLETED") => {
            return detail(StatusCode::BAD_REQUEST, "Backup is not completed");
        }
        Ok(Some((_, _, Some(true)))) => {
            return detail(StatusCode::BAD_REQUEST, "Private key required");
        }
        Ok(Some((_, path, _))) => path.unwrap_or_default(),
        Err(_) => return internal(),
    };
    let busy = || {
        detail(
            StatusCode::SERVICE_UNAVAILABLE,
            "Download capacity exhausted",
        )
    };
    let Ok(slot) = state.download_slots.clone().try_acquire_owned() else {
        return busy();
    };
    let slot = Arc::new(slot);
    let job_slot = slot.clone();
    let root = state.config.backup_base_path.clone();
    let name =
        String::from_utf8_lossy(normalized.rsplit(|&c| c == b'/').next().unwrap_or_default());
    let disposition_name: String = name
        .chars()
        .take(255)
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '.' | '_' | '-') {
                c
            } else {
                '_'
            }
        })
        .collect();
    let disposition = format!(
        "attachment; filename=\"{}\"",
        if disposition_name.is_empty() {
            "download"
        } else {
            &disposition_name
        }
    );
    let Ok(job) = state.download_jobs.submit_blocking(move |cancel| {
        let _slot = job_slot;
        with_local_archive(&root, &path, &|| cancel.is_cancelled(), |source, root| {
            let Some(member) = resolve(source, normalized, &|| cancel.is_cancelled())? else {
                return Ok(None);
            };
            if member.kind == MemberKind::Directory {
                return Ok(Some(Err(())));
            }
            let mut file = temp(root)?;
            source.seek(SeekFrom::Start(member.data_offset))?;
            let mut remaining = member.size;
            let mut buf = [0; 65536];
            while remaining != 0 {
                if cancel.is_cancelled() {
                    return Err(io::ErrorKind::Interrupted.into());
                }
                let count = remaining.min(buf.len() as u64) as usize;
                source.read_exact(&mut buf[..count])?;
                file.write_all(&buf[..count])?;
                remaining -= count as u64;
            }
            file.rewind()?;
            let validators = Validators::read(&file, member.size)?;
            Ok(Some(Ok((file, member.size, validators))))
        })
    }) else {
        return busy();
    };
    match job.wait().await {
        Ok(Ok(Some(Ok((file, len, validators))))) => {
            let mut result = response(
                file,
                len,
                slot,
                disposition,
                validators,
                &HeaderMap::new(),
                "application/octet-stream",
            );
            result
                .headers_mut()
                .insert("cache-control", HeaderValue::from_static("no-store"));
            result
        }
        Ok(Ok(Some(Err(())))) => detail(StatusCode::BAD_REQUEST, "Cannot download directories"),
        Ok(Ok(None)) => detail(StatusCode::NOT_FOUND, "File not found in archive"),
        Ok(Err(error)) if error.kind() == io::ErrorKind::NotFound => detail(
            StatusCode::NOT_FOUND,
            "Backup file not found locally or on any remote storage.",
        ),
        _ => unreadable(),
    }
}

fn normalize(path: &[u8]) -> Vec<u8> {
    let absolute = path.starts_with(b"/");
    let mut parts: Vec<&[u8]> = Vec::new();
    for part in path.split(|&c| c == b'/') {
        match part {
            b"" | b"." => {}
            b".." if parts.last().is_some_and(|p| *p != b"..") => {
                parts.pop();
            }
            b".." if absolute => {}
            _ => parts.push(part),
        }
    }
    let mut normalized = if absolute {
        if path.starts_with(b"//") && !path.starts_with(b"///") {
            b"//".to_vec()
        } else {
            b"/".to_vec()
        }
    } else {
        Vec::new()
    };
    normalized.extend(parts.join(&b'/'));
    if normalized.is_empty() {
        normalized.push(b'.');
    }
    normalized
}

fn resolve(
    source: &mut File,
    mut name: Vec<u8>,
    cancelled: &dyn Fn() -> bool,
) -> io::Result<Option<Member>> {
    let mut before = None;
    let mut seen = HashSet::new();
    for depth in 0..1000 {
        if !seen.insert((Sha256::digest(&name), before, depth == 0)) {
            break;
        }
        let mut selected = None;
        for member in ArchiveReader::new(&mut *source)? {
            if cancelled() {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let member = member?;
            if before.is_some_and(|offset| member.data_offset >= offset) {
                break;
            }
            if (depth == 0 && member.name == name)
                || (depth != 0 && normalize(&member.name) == name)
            {
                selected = Some(member);
            }
        }
        let Some(member) = selected else {
            return Ok(None);
        };
        if !matches!(member.kind, MemberKind::Hardlink | MemberKind::Symlink) {
            return Ok(Some(member));
        }
        let Some(link) = member.link_name else {
            return Ok(None);
        };
        before = (member.kind == MemberKind::Hardlink).then_some(member.data_offset);
        name = if member.kind == MemberKind::Symlink && !link.is_empty() {
            match member.name.iter().rposition(|&c| c == b'/') {
                Some(i) if i != 0 => normalize(&[&member.name[..i], link.as_slice()].join(&b'/')),
                Some(_) => normalize(&[b"/".as_slice(), link.as_slice()].join(&b'/')),
                None => normalize(&link),
            }
        } else {
            normalize(&link)
        };
    }
    Ok(None)
}
