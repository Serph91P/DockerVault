use super::HttpState;
use axum::{
    Json,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::json;
use std::{
    io::Read,
    os::{fd::AsRawFd, unix::fs::MetadataExt},
    path::Path as FsPath,
    time::Duration,
};

const KEY_ROOT: &str = "/app/data/ssh_keys";

fn valid_name(name: &str) -> bool {
    (1..=40).contains(&name.len())
        && name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
}

fn bad_name() -> Response {
    failure(
        StatusCode::BAD_REQUEST,
        "Key name must be 1-40 chars: letters, digits, '_' or '-'.",
    )
}

fn unavailable() -> Response {
    failure(
        StatusCode::SERVICE_UNAVAILABLE,
        "SSH key metadata unavailable",
    )
}

pub(super) async fn list(State(state): State<HttpState>) -> Response {
    let Ok(work) = state.auth.blocking().submit_blocking(move |cancel| {
        let mut names = Vec::new();
        if let Ok(root) = crate::storage::local::open_root(FsPath::new(KEY_ROOT))
            && let Ok(entries) = std::fs::read_dir(format!("/proc/self/fd/{}", root.as_raw_fd()))
        {
            for entry in entries.flatten() {
                if cancel.is_cancelled() {
                    break;
                }
                if let Some(file) = entry
                    .file_name()
                    .to_str()
                    .and_then(|s| s.strip_suffix(".pub"))
                    && valid_name(file)
                {
                    names.push(file.to_owned());
                }
            }
        }
        names.sort_by_cached_key(|name| format!("{name}.pub"));
        names
    }) else {
        return unavailable();
    };
    let Ok(names) = work.wait().await else {
        return unavailable();
    };
    let mut result = Vec::new();
    for name in names {
        match metadata(&state, name).await {
            Ok(Some(value)) => result.push(value),
            Ok(None) => (),
            Err(()) => return unavailable(),
        }
    }
    Json(result).into_response()
}

pub(super) async fn detail(State(state): State<HttpState>, Path(name): Path<String>) -> Response {
    if !valid_name(&name) {
        return bad_name();
    }
    match metadata(&state, name).await {
        Ok(Some(value)) => Json(value).into_response(),
        Ok(None) => failure(StatusCode::NOT_FOUND, "SSH key not found"),
        Err(()) => unavailable(),
    }
}

async fn metadata(state: &HttpState, name: String) -> Result<Option<serde_json::Value>, ()> {
    let key_name = name.clone();
    let work = state
        .auth
        .blocking()
        .submit_blocking(move |_| {
            let root = FsPath::new(KEY_ROOT);
            let private =
                crate::storage::local::open_download_source(root, FsPath::new(&key_name)).ok()?;
            let metadata = private.metadata().ok()?;
            let created_at =
                metadata.mtime() as f64 + metadata.mtime_nsec() as f64 / 1_000_000_000.0;
            let public = read_public(root, FsPath::new(&format!("{key_name}.pub")))?;
            Some((String::from_utf8(public).ok()?, created_at))
        })
        .map_err(|_| ())?;
    let Some((public, created_at)) = work.wait().await.map_err(|_| ())? else {
        return Ok(None);
    };
    let mut fingerprint = None;
    if let Some(processes) = state.probe_processes.as_ref() {
        let mut spec =
            crate::process::ProcessSpec::new("/usr/bin/ssh-keygen", ["-lf", "/dev/stdin"]);
        spec.input = public.as_bytes().to_vec();
        spec.timeout = Some(Duration::from_secs(5));
        if let Ok(work) = processes.start(spec)
            && let Ok(Ok(output)) = work.wait().await
            && output.status.success()
        {
            fingerprint = Some(String::from_utf8_lossy(&output.stdout).trim().to_owned());
        }
    }
    Ok(Some(json!({
        "name": name,
        "private_path": format!("{KEY_ROOT}/{name}"),
        "public_path": format!("{KEY_ROOT}/{name}.pub"),
        "public_key": public.trim(),
        "fingerprint": fingerprint,
        "created_at": created_at,
    })))
}

pub(super) async fn download_public(
    State(state): State<HttpState>,
    Path(name): Path<String>,
) -> Response {
    if !valid_name(&name) {
        return bad_name();
    }
    let filename = format!("{name}.pub");
    let disposition = format!("attachment; filename=\"{filename}\"");
    let task = state
        .auth
        .blocking()
        .submit_blocking(move |_| read_public(FsPath::new(KEY_ROOT), FsPath::new(&filename)));
    if let Ok(task) = task {
        match task.wait().await {
            Ok(Some(bytes)) => {
                return (
                    [
                        (header::CONTENT_TYPE, "text/plain; charset=utf-8".to_owned()),
                        (header::CONTENT_DISPOSITION, disposition),
                    ],
                    bytes,
                )
                    .into_response();
            }
            Ok(None) => return failure(StatusCode::NOT_FOUND, "Public key not found"),
            Err(_) => {}
        }
    }
    failure(
        StatusCode::SERVICE_UNAVAILABLE,
        "Public key download unavailable",
    )
}

pub(super) fn export_public(
    restored: Option<&str>,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<Vec<serde_json::Value>, ()> {
    use std::collections::BTreeMap;
    #[derive(serde::Deserialize)]
    struct PublicKey {
        name: String,
        public_key: String,
    }
    let restored: Vec<PublicKey> =
        serde_json::from_str(restored.unwrap_or("[]")).map_err(|_| ())?;
    let mut keys = BTreeMap::new();
    for PublicKey { name, public_key } in restored {
        let key = dockervault_core::ssh_public::canonical_public_key(public_key.as_bytes())
            .map_err(|_| ())?;
        keys.insert(name, key);
    }
    let root = FsPath::new(KEY_ROOT);
    if root.exists() {
        let dir = crate::storage::local::open_root(root).map_err(|_| ())?;
        for entry in
            std::fs::read_dir(format!("/proc/self/fd/{}", dir.as_raw_fd())).map_err(|_| ())?
        {
            if cancel.is_cancelled() {
                return Err(());
            }
            let entry = entry.map_err(|_| ())?;
            let filename = entry.file_name();
            let Some(name) = filename
                .to_str()
                .and_then(|name| name.strip_suffix(".pub"))
                .filter(|name| valid_name(name))
            else {
                continue;
            };
            if entry.file_type().map_err(|_| ())?.is_symlink() {
                continue;
            }
            let file = crate::storage::local::open_download_source(root, FsPath::new(&filename))
                .map_err(|_| ())?;
            let mut bytes = Vec::new();
            file.take(16_385).read_to_end(&mut bytes).map_err(|_| ())?;
            if bytes.len() > 16_384 {
                return Err(());
            }
            let key = dockervault_core::ssh_public::canonical_public_key(&bytes).map_err(|_| ())?;
            keys.insert(name.to_owned(), key);
        }
    }
    Ok(keys
        .into_iter()
        .map(|(name, public_key)| json!({"name":name,"public_key":public_key}))
        .collect())
}

fn read_public(root: &FsPath, filename: &FsPath) -> Option<Vec<u8>> {
    let file = crate::storage::local::open_download_source(root, filename).ok()?;
    let mut bytes = Vec::new();
    file.take(16_385).read_to_end(&mut bytes).ok()?;
    let line = bytes
        .strip_suffix(b"\r\n")
        .or_else(|| bytes.strip_suffix(b"\n"))
        .unwrap_or(&bytes);
    if bytes.len() > 16_384 || line.iter().any(|b| matches!(b, b'\n' | b'\r')) {
        return None;
    }
    dockervault_core::ssh_public::canonical_public_key(line).ok()?;
    Some(bytes)
}

fn failure(status: StatusCode, detail: &str) -> Response {
    (status, Json(json!({"detail":detail}))).into_response()
}
