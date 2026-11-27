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
const KEY_JOURNAL: &str = "native_ssh_key_change";

#[derive(serde::Deserialize)]
pub(super) struct Generate {
    name: String,
    comment: Option<String>,
    #[serde(default)]
    overwrite: bool,
}

pub(super) async fn generate(
    State(state): State<HttpState>,
    body: Result<Json<Generate>, axum::extract::rejection::JsonRejection>,
) -> Response {
    let Ok(Json(input)) = body else {
        return failure(StatusCode::UNPROCESSABLE_ENTITY, "Invalid SSH key request");
    };
    if !valid_name(&input.name) {
        return bad_name();
    }
    let worker = state.clone();
    let Ok(work) = state
        .probe_jobs
        .submit_async(move |cancel| async move { create_new(&worker, input, &cancel).await })
    else {
        return unavailable();
    };
    match work.wait().await {
        Ok(Ok(value)) => Json(value).into_response(),
        Ok(Err((status, detail))) => failure(status, detail),
        Err(_) => unavailable(),
    }
}

async fn create_new(
    state: &HttpState,
    input: Generate,
    cancel: &tokio_util::sync::CancellationToken,
) -> Result<serde_json::Value, (StatusCode, &'static str)> {
    const ERROR: (StatusCode, &str) = (
        StatusCode::INTERNAL_SERVER_ERROR,
        "Could not generate SSH key",
    );
    const CONFLICT: (StatusCode, &str) =
        (StatusCode::CONFLICT, "Key with this name already exists");
    let Generate {
        name,
        comment,
        overwrite,
    } = input;
    let comment = comment
        .filter(|c| !c.is_empty())
        .unwrap_or_else(|| format!("dockervault-{name}"));
    if comment.len() > 1024 || comment.contains(['\0', '\n', '\r']) {
        return Err((StatusCode::BAD_REQUEST, "Invalid SSH key comment"));
    }
    let _claim = state.ssh_key_write.lock().await;
    recover_key_change(state.auth.pool())
        .await
        .map_err(|_| ERROR)?;
    if cancel.is_cancelled() {
        return Err(ERROR);
    }
    let public = format!("{name}.pub");
    let root = FsPath::new(KEY_ROOT);
    let dir = crate::storage::local::open_root(root).map_err(|_| ERROR)?;
    let anchored = std::path::PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd()));
    let old = [
        stamp(&anchored.join(&name)).map_err(|_| ERROR)?,
        stamp(&anchored.join(&public)).map_err(|_| ERROR)?,
    ];
    if !overwrite && old.iter().any(Option::is_some) {
        return Err(CONFLICT);
    }
    let mut random = [0u8; 16];
    getrandom::fill(&mut random).map_err(|_| ERROR)?;
    let stage_name = format!(".dv-key-{}", u128::from_ne_bytes(random));
    nix::sys::stat::mkdirat(&dir, stage_name.as_str(), nix::sys::stat::Mode::S_IRWXU)
        .map_err(|_| ERROR)?;
    let mut stage = KeyStage(Some(anchored.join(&stage_name)));
    let mut spec = crate::process::ProcessSpec::new(
        "/usr/bin/ssh-keygen",
        ["-q", "-t", "ed25519", "-N", "", "-C", &comment, "-f", "key"],
    );
    spec.working_directory = Some(root.join(&stage_name));
    spec.timeout = Some(Duration::from_secs(10));
    spec.discard_output = true;
    let work = state
        .probe_processes
        .as_ref()
        .ok_or(ERROR)?
        .start(spec)
        .map_err(|_| ERROR)?;
    let output = work.wait().await.map_err(|_| ERROR)?.map_err(|_| ERROR)?;
    if !output.status.success() || cancel.is_cancelled() {
        return Err(ERROR);
    }
    use nix::{
        fcntl::{OFlag, openat},
        sys::stat::Mode,
    };
    let stage_dir = openat(
        &dir,
        stage_name.as_str(),
        OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC,
        Mode::empty(),
    )
    .map_err(|_| ERROR)?;
    let open = |name| {
        let file = std::fs::File::from(
            openat(
                &stage_dir,
                name,
                OFlag::O_RDONLY | OFlag::O_NOFOLLOW | OFlag::O_NONBLOCK | OFlag::O_CLOEXEC,
                Mode::empty(),
            )
            .map_err(|_| ERROR)?,
        );
        if file.metadata().map_err(|_| ERROR)?.is_file() {
            Ok(file)
        } else {
            Err(ERROR)
        }
    };
    let private = open("key")?;
    let generated_public = open("key.pub")?;
    use std::os::unix::fs::PermissionsExt;
    private
        .set_permissions(std::fs::Permissions::from_mode(0o600))
        .map_err(|_| ERROR)?;
    generated_public
        .set_permissions(std::fs::Permissions::from_mode(0o644))
        .map_err(|_| ERROR)?;
    private.sync_all().map_err(|_| ERROR)?;
    generated_public.sync_all().map_err(|_| ERROR)?;
    std::fs::File::from(stage_dir)
        .sync_all()
        .map_err(|_| ERROR)?;
    dir.sync_all().map_err(|_| ERROR)?;
    if cancel.is_cancelled() {
        return Err(ERROR);
    }
    let change = KeyChange {
        name: name.clone(),
        stage: Some(stage_name),
        old,
        new: Some([
            identity(&private).map_err(|_| ERROR)?,
            identity(&generated_public).map_err(|_| ERROR)?,
        ]),
    };
    let encoded = serde_json::to_string(&change).map_err(|_| ERROR)?;
    sqlx::query("INSERT INTO app_settings(key,value) VALUES(?,?)")
        .bind(KEY_JOURNAL)
        .bind(&encoded)
        .execute(state.auth.pool())
        .await
        .map_err(|_| ERROR)?;
    // Once the decision is durable, restart (not request cancellation) owns settlement.
    stage.0 = None;
    settle_key_change(state.auth.pool(), &encoded, change)
        .await
        .map_err(|_| ERROR)?;
    metadata(state, name).await.map_err(|_| ERROR)?.ok_or(ERROR)
}

struct KeyStage(Option<std::path::PathBuf>);

impl Drop for KeyStage {
    fn drop(&mut self) {
        if let Some(path) = &self.0 {
            let _ = std::fs::remove_file(path.join("key"));
            let _ = std::fs::remove_file(path.join("key.pub"));
            let _ = std::fs::remove_dir(path);
        }
    }
}

type Stamp = (u64, u64);

#[derive(serde::Deserialize, serde::Serialize)]
struct KeyChange {
    name: String,
    stage: Option<String>,
    old: [Option<Stamp>; 2],
    new: Option<[Stamp; 2]>,
}

pub(super) async fn delete(State(state): State<HttpState>, Path(name): Path<String>) -> Response {
    if !valid_name(&name) {
        return bad_name();
    }
    let ctx = state.clone();
    let Ok(work) = state.probe_jobs.submit_async(move |cancel| async move {
        let _claim = ctx.ssh_key_write.lock().await;
        let pool = ctx.auth.pool();
        recover_key_change(pool).await.map_err(|_| ())?;
        if cancel.is_cancelled() {
            return Err(());
        }
        let dir = match crate::storage::local::open_root(FsPath::new(KEY_ROOT)) {
            Ok(dir) => dir,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(false),
            Err(_) => return Err(()),
        };
        let root = std::path::PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd()));
        let old = [
            stamp(&root.join(&name)).map_err(|_| ())?,
            stamp(&root.join(format!("{name}.pub"))).map_err(|_| ())?,
        ];
        if old.iter().all(Option::is_none) {
            return Ok(false);
        }
        let change = KeyChange {
            name,
            stage: None,
            old,
            new: None,
        };
        let encoded = serde_json::to_string(&change).map_err(|_| ())?;
        sqlx::query("INSERT INTO app_settings(key,value) VALUES(?,?)")
            .bind(KEY_JOURNAL)
            .bind(&encoded)
            .execute(pool)
            .await
            .map_err(|_| ())?;
        settle_key_change(pool, &encoded, change).await?;
        Ok(true)
    }) else {
        return unavailable();
    };
    match work.wait().await {
        Ok(Ok(true)) => Json(json!({"success":true})).into_response(),
        Ok(Ok(false)) => failure(StatusCode::NOT_FOUND, "SSH key not found"),
        Ok(Err(())) => failure(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not delete SSH key",
        ),
        Err(_) => unavailable(),
    }
}

fn identity(file: &std::fs::File) -> std::io::Result<Stamp> {
    file.metadata().map(|m| (m.dev(), m.ino()))
}

fn stamp(path: &FsPath) -> std::io::Result<Option<Stamp>> {
    match std::fs::symlink_metadata(path) {
        Ok(m) if m.is_file() => Ok(Some((m.dev(), m.ino()))),
        Ok(_) => Err(std::io::Error::other("Not a regular key file")),
        Err(err) if err.kind() == std::io::ErrorKind::NotFound => Ok(None),
        Err(err) => Err(err),
    }
}

pub async fn recover_key_change(pool: &sqlx::SqlitePool) -> Result<(), sqlx::Error> {
    let row: Option<String> = sqlx::query_scalar("SELECT value FROM app_settings WHERE key=?")
        .bind(KEY_JOURNAL)
        .fetch_optional(pool)
        .await?;
    if let Some(row) = row {
        let change = serde_json::from_str(&row)
            .map_err(|_| sqlx::Error::Protocol("Invalid SSH key publication journal".into()))?;
        settle_key_change(pool, &row, change)
            .await
            .map_err(|_| sqlx::Error::Protocol("SSH key publication remains unsettled".into()))?;
    }
    Ok(())
}

async fn settle_key_change(
    pool: &sqlx::SqlitePool,
    encoded: &str,
    change: KeyChange,
) -> Result<(), ()> {
    let owner = tokio::task::spawn_blocking(move || publish_keys(&change))
        .await
        .map_err(|_| ())??;
    let changed = sqlx::query("DELETE FROM app_settings WHERE key=? AND value=?")
        .bind(KEY_JOURNAL)
        .bind(encoded)
        .execute(pool)
        .await
        .map_err(|_| ())?
        .rows_affected();
    if changed != 1 {
        return Err(());
    }
    // Stage paths are anchored to the held descriptor until cleanup finishes.
    drop(KeyStage(owner.1));
    drop(owner.0);
    Ok(())
}

fn publish_keys(change: &KeyChange) -> Result<(std::fs::File, Option<std::path::PathBuf>), ()> {
    if !valid_name(&change.name) {
        return Err(());
    }
    let dir = crate::storage::local::open_root(FsPath::new(KEY_ROOT)).map_err(|_| ())?;
    let root = std::path::PathBuf::from(format!("/proc/self/fd/{}", dir.as_raw_fd()));
    let public = format!("{}.pub", change.name);
    let pairs = [("key", change.name.as_str()), ("key.pub", public.as_str())];
    if change.stage.is_none() && change.new.is_none() {
        for (idx, (_, destination)) in pairs.iter().enumerate() {
            let current = stamp(&root.join(destination)).map_err(|_| ())?;
            if current.is_some() && current != change.old[idx] {
                return Err(());
            }
        }
        for (idx, (_, destination)) in pairs.iter().enumerate() {
            let dst = root.join(destination);
            match stamp(&dst).map_err(|_| ())? {
                None => (),
                current if current == change.old[idx] => {
                    std::fs::remove_file(dst).map_err(|_| ())?
                }
                _ => return Err(()),
            }
        }
        dir.sync_all().map_err(|_| ())?;
        return Ok((dir, None));
    }
    let (Some(stage_name), Some(new)) = (&change.stage, change.new) else {
        return Err(());
    };
    if !stage_name.strip_prefix(".dv-key-").is_some_and(|suffix| {
        !suffix.is_empty() && suffix.len() <= 39 && suffix.bytes().all(|b| b.is_ascii_digit())
    }) {
        return Err(());
    }
    let stage = root.join(stage_name);
    let stage_id = std::fs::symlink_metadata(&stage).map_err(|_| ())?;
    if !stage_id.is_dir() {
        return Err(());
    }
    // Check both sides before modifying either. A previously installed inode is already settled.
    for (idx, (source, destination)) in pairs.iter().enumerate() {
        let current = stamp(&root.join(destination)).map_err(|_| ())?;
        if current != Some(new[idx])
            && (current != change.old[idx]
                || stamp(&stage.join(source)).map_err(|_| ())? != Some(new[idx]))
        {
            return Err(());
        }
    }
    for (idx, (source, destination)) in pairs.iter().enumerate() {
        let dst = root.join(destination);
        if stamp(&dst).map_err(|_| ())? == Some(new[idx]) {
            continue;
        }
        let src = stage.join(source);
        if change.old[idx].is_some() {
            std::fs::rename(src, dst).map_err(|_| ())?;
        } else {
            std::fs::hard_link(src, dst).map_err(|_| ())?;
        }
    }
    dir.sync_all().map_err(|_| ())?;
    Ok((dir, Some(stage)))
}

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

#[derive(serde::Deserialize)]
pub(super) struct Install {
    host: String,
    port: Option<u16>,
    username: String,
    password: String,
    method: Option<String>,
}

pub(super) async fn install(
    State(state): State<HttpState>,
    Path(name): Path<String>,
    body: Result<Json<Install>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if !valid_name(&name) {
        return bad_name();
    }
    let Ok(Json(input)) = body else {
        return failure(
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid SSH install request",
        );
    };
    let filename = format!("{name}.pub");
    let Ok(read) = state
        .auth
        .blocking()
        .submit_blocking(move |_| read_public(FsPath::new(KEY_ROOT), FsPath::new(&filename)))
    else {
        return unavailable();
    };
    let mut key = match read.wait().await {
        Ok(Some(bytes)) => bytes,
        Ok(None) => return failure(StatusCode::NOT_FOUND, "Public key not found"),
        Err(_) => return unavailable(),
    };
    if !key.ends_with(b"\n") {
        key.push(b'\n');
    }
    let port = input.port.unwrap_or(22);
    let method = match input.method.as_deref().unwrap_or("auto") {
        "auto" if port == 23 && input.host.ends_with(".your-storagebox.de") => "hetzner",
        "hetzner" => "hetzner",
        "auto" | "authorized_keys" => "authorized_keys",
        other => {
            let detail = if valid_name(other) && !input.password.contains(other) {
                format!("Unknown install method: {other}")
            } else {
                "Unknown install method".to_owned()
            };
            return failure(StatusCode::BAD_REQUEST, &detail);
        }
    };
    if input.password.is_empty() || input.password.len() > 4096 || input.password.contains('\0') {
        return failure(StatusCode::BAD_REQUEST, "Invalid SSH install password");
    }
    let endpoint = crate::storage::ssh::SftpEndpoint {
        host: input.host,
        username: input.username,
        port,
        base: String::new(),
        known_hosts: "/app/data/ssh_known_hosts".into(),
        identity: None,
        password: Some(input.password),
    };
    let Some(processes) = state.probe_processes.clone() else {
        return unavailable();
    };
    let command = if method == "hetzner" {
        "install-ssh-key"
    } else {
        concat!(
            "set -e; umask 077; mkdir -p ~/.ssh; chmod 700 ~/.ssh; ",
            "touch ~/.ssh/authorized_keys; chmod 600 ~/.ssh/authorized_keys; ",
            "key=$(cat); grep -qxF -- \"$key\" ~/.ssh/authorized_keys || ",
            "printf '%s\\n' \"$key\" >> ~/.ssh/authorized_keys"
        )
    };
    let Ok(work) = state.probe_jobs.submit_async(move |cancel| async move {
        endpoint
            .install_public_key(&processes, command, key, &cancel)
            .await
    }) else {
        return unavailable();
    };
    match work.wait().await {
        Ok(Ok(())) => {
            Json(json!({"success":true,"method":method,"message":"Key installed"})).into_response()
        }
        Ok(Err(_)) => failure(
            StatusCode::BAD_REQUEST,
            "Install failed: SSH did not confirm key installation",
        ),
        Err(_) => unavailable(),
    }
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
