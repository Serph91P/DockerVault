use super::HttpState;
use axum::{
    Json,
    extract::{Path, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use serde_json::json;
use std::{io::Read, path::Path as FsPath};

const KEY_ROOT: &str = "/app/data/ssh_keys";

pub(super) async fn download_public(
    State(state): State<HttpState>,
    Path(name): Path<String>,
) -> Response {
    if !(1..=40).contains(&name.len())
        || !name
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'_' | b'-'))
    {
        return failure(
            StatusCode::BAD_REQUEST,
            "Key name must be 1-40 chars: letters, digits, '_' or '-'.",
        );
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
