use super::{
    Filters, HttpState, INTEGER, detail, internal, query_integer, required_user, sanitized_path,
    validation,
};
use crate::{
    restore::{ArchiveReader, MemberKind},
    storage::local::open_download_source,
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use dockervault_core::crypto::{decrypt_dek, decrypt_private, key_sidecar_path};
use serde_json::{Value, json};
use std::{
    collections::HashMap,
    fs::File,
    io::{self, Read, Seek, Write},
    os::unix::fs::MetadataExt,
    path::Path as FsPath,
};
use time::{OffsetDateTime, macros::format_description};
use zeroize::Zeroizing;
#[path = "backup_browse_member.rs"]
mod member_route;
pub(in crate::http) use member_route::{member, member_encrypted};

const MAX_PAGE: i64 = 2000;

#[derive(serde::Deserialize)]
pub(in crate::http) struct BrowseKey {
    #[serde(deserialize_with = "read_private_key")]
    private_key: Zeroizing<String>,
}
fn read_private_key<'de, D: serde::Deserializer<'de>>(
    input: D,
) -> Result<Zeroizing<String>, D::Error> {
    <String as serde::Deserialize>::deserialize(input).map(Zeroizing::new)
}
fn invalid_key_body() -> Response {
    *validation(vec![
        json!({"field":"body.private_key","message":"Field required"}),
    ])
}
fn key_error() -> Response {
    detail(
        StatusCode::BAD_REQUEST,
        "Unable to decrypt backup with the provided private key",
    )
}

pub(in crate::http) async fn list(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(values): Query<HashMap<String, String>>,
) -> Response {
    if let Err(error) = required_user(&state, &headers).await {
        return *error;
    }
    list_with_key(state, id, values, None).await
}

pub(in crate::http) async fn list_encrypted(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(id): Path<String>,
    Query(values): Query<HashMap<String, String>>,
    input: Result<Json<BrowseKey>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if let Err(error) = required_user(&state, &headers).await {
        return *error;
    }
    let Ok(Json(key)) = input else {
        return invalid_key_body();
    };
    list_with_key(state, id, values, Some(key)).await
}

async fn list_with_key(
    state: HttpState,
    id: String,
    values: HashMap<String, String>,
    secret: Option<BrowseKey>,
) -> Response {
    let mut query = Filters {
        values,
        errors: Vec::new(),
    };
    let id = query_integer(&id);
    if id.is_none() {
        query
            .errors
            .push(json!({"field":"path.backup_id","message":INTEGER}));
    }
    let limit = query
        .integer("limit", Some(1), Some(MAX_PAGE))
        .unwrap_or(MAX_PAGE) as u64;
    let offset = query.integer("offset", Some(0), None).unwrap_or(0) as u64;
    let search = query.values.get("search").cloned().unwrap_or_default();
    if search.chars().count() > 200 {
        query.error("search", "String should have at most 200 characters");
    }
    if !query.errors.is_empty() {
        return *validation(query.errors);
    }
    let current =
        sqlx::query_as::<_, (Option<String>, Option<String>, Option<bool>, Option<String>)>(
            "SELECT status,file_path,encrypted,encryption_key_path FROM backups WHERE id=?",
        )
        .bind(id)
        .fetch_optional(state.auth.pool())
        .await;
    let (status, path, encrypted, sidecar) = match current {
        Ok(Some(value)) => value,
        Ok(None) => return detail(StatusCode::NOT_FOUND, "Backup not found"),
        Err(_) => return internal(),
    };
    if status.as_deref() != Some("COMPLETED") {
        return detail(StatusCode::BAD_REQUEST, "Backup is not completed");
    }
    if encrypted.unwrap_or(false) != secret.is_some() {
        return detail(
            StatusCode::BAD_REQUEST,
            if secret.is_some() {
                "Backup is not encrypted. Use GET endpoint instead."
            } else {
                "Private key required"
            },
        );
    }
    let unavailable = || {
        detail(
            StatusCode::NOT_FOUND,
            "Backup file not found locally or on any remote storage.",
        )
    };
    let Some(path) = path.filter(|value| !value.is_empty()) else {
        return unavailable();
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
    let root = state.config.backup_base_path.clone();
    let Ok(job) = state.download_jobs.submit_blocking(move |cancel| {
        let _slot = slot;
        let encryption = secret
            .as_ref()
            .map(|key| (sidecar.as_deref(), key.private_key.as_str()));
        with_local_archive(
            &root,
            &path,
            encryption,
            &|| cancel.is_cancelled(),
            |source, _| {
                scan(source, offset, limit, &search.to_lowercase(), || {
                    cancel.is_cancelled()
                })
            },
        )
    }) else {
        return busy();
    };
    match job.wait().await {
        Ok(Ok((entries, total))) => {
            let mut response = Json(entries).into_response();
            response
                .headers_mut()
                .insert("x-total-count", HeaderValue::from(total));
            response.headers_mut().insert(
                "x-has-more",
                HeaderValue::from_static(if offset + limit < total {
                    "true"
                } else {
                    "false"
                }),
            );
            response
        }
        Ok(Err(error)) if error.kind() == io::ErrorKind::NotFound => unavailable(),
        Ok(Err(error)) if error.kind() == io::ErrorKind::PermissionDenied => key_error(),
        _ => unreadable(),
    }
}

fn unreadable() -> Response {
    detail(
        StatusCode::INTERNAL_SERVER_ERROR,
        "Failed to read backup archive",
    )
}

fn temp(root: &FsPath) -> io::Result<File> {
    let fd = nix::fcntl::open(
        root,
        nix::fcntl::OFlag::O_TMPFILE | nix::fcntl::OFlag::O_RDWR | nix::fcntl::OFlag::O_CLOEXEC,
        nix::sys::stat::Mode::S_IRUSR | nix::sys::stat::Mode::S_IWUSR,
    )?;
    Ok(File::from(fd))
}

fn with_local_archive<T>(
    root: &str,
    path: &str,
    encryption: Option<(Option<&str>, &str)>,
    cancelled: &dyn Fn() -> bool,
    task: impl FnOnce(&mut File, &FsPath) -> io::Result<T>,
) -> io::Result<T> {
    if root.is_empty() {
        return Err(io::ErrorKind::NotFound.into());
    }
    let root = std::path::absolute(root)?;
    let open = |path: &str| {
        let absolute = std::path::absolute(path)?;
        let relative = absolute
            .strip_prefix(&root)
            .map_err(|_| io::ErrorKind::NotFound)?;
        open_download_source(&root, relative)
    };
    let (mut source, path) = match open(path) {
        Ok(file) => (file, path.to_owned()),
        Err(e) if e.kind() == io::ErrorKind::NotFound => {
            let fallback = sanitized_path(path);
            if fallback == path {
                return Err(e);
            }
            (open(&fallback)?, fallback)
        }
        Err(e) => return Err(e),
    };
    let before = identity(&source)?;
    let archive_name = path.strip_suffix(".enc").unwrap_or(&path);
    let compressed = archive_name.ends_with(".tar.gz") || archive_name.ends_with(".tgz");
    let mut decrypted;
    let input = if let Some((key_path, secret)) = encryption {
        let fallback = key_sidecar_path(&path);
        let key_path = key_path.filter(|p| !p.is_empty()).unwrap_or(&fallback);
        let key = open(key_path).map_err(|_| io::ErrorKind::PermissionDenied)?;
        let size = key.metadata()?.len();
        if !(1..=65536).contains(&size) {
            return Err(io::ErrorKind::PermissionDenied.into());
        }
        let material =
            decrypt_dek(key.take(65537), secret).map_err(|_| io::ErrorKind::PermissionDenied)?;
        decrypted = decrypt_private(
            source.try_clone()?,
            before.2,
            temp(&root)?,
            &material,
            cancelled,
        )
        .map_err(|_| io::ErrorKind::PermissionDenied)?;
        &mut decrypted
    } else {
        &mut source
    };
    let mut spool;
    let input = if compressed {
        spool = temp(&root)?;
        let mut gzip = flate2::read::MultiGzDecoder::new(input);
        let mut bytes = [0; 65536];
        loop {
            if cancelled() {
                return Err(io::ErrorKind::Interrupted.into());
            }
            let len = gzip.read(&mut bytes)?;
            if len == 0 {
                break;
            }
            spool.write_all(&bytes[..len])?;
        }
        spool.rewind()?;
        &mut spool
    } else {
        input
    };
    let result = task(input, &root)?;
    if identity(&source)? != before || identity(&open(&path)?)? != before {
        return Err(io::ErrorKind::InvalidData.into());
    }
    Ok(result)
}

fn identity(file: &File) -> io::Result<(u64, u64, u64, i64, i64, i64, i64)> {
    let meta = file.metadata()?;
    Ok((
        meta.dev(),
        meta.ino(),
        meta.size(),
        meta.mtime(),
        meta.mtime_nsec(),
        meta.ctime(),
        meta.ctime_nsec(),
    ))
}

fn scan(
    source: &mut File,
    offset: u64,
    limit: u64,
    search: &str,
    cancelled: impl Fn() -> bool,
) -> io::Result<(Vec<Value>, u64)> {
    let mut page = Vec::new();
    let mut total = 0;
    for member in ArchiveReader::new(source)? {
        if cancelled() {
            return Err(io::ErrorKind::Interrupted.into());
        }
        let member = member?;
        let path = String::from_utf8_lossy(&member.name);
        if !search.is_empty() && !path.to_lowercase().contains(search) {
            continue;
        }
        if offset <= total && total < offset + limit {
            let name = FsPath::new(path.as_ref())
                .file_name()
                .and_then(|name| name.to_str())
                .unwrap_or(&path);
            let mtime = if member.mtime == 0.0 {
                String::new()
            } else {
                let stamp = OffsetDateTime::from_unix_timestamp_nanos(
                    (member.mtime * 1e6).round() as i128 * 1000,
                )
                .map_err(io::Error::other)?;
                let mut value = stamp
                    .format(format_description!(
                        "[year]-[month]-[day]T[hour]:[minute]:[second]"
                    ))
                    .map_err(io::Error::other)?;
                if stamp.microsecond() != 0 {
                    value.push_str(&format!(".{:06}", stamp.microsecond()));
                }
                value
            };
            page.push(json!({"name":name,"path":path,"size":member.size,"size_human":human(member.size),"is_dir":member.kind == MemberKind::Directory,"mode":format!("{:o}",member.mode),"mtime":mtime}));
        }
        total += 1;
    }
    Ok((page, total))
}

fn human(size: u64) -> String {
    let mut size = size as f64;
    for unit in ["B", "KB", "MB", "GB", "TB"] {
        if size < 1024.0 {
            return format!("{size:.1} {unit}");
        }
        size /= 1024.0;
    }
    format!("{size:.1} PB")
}
