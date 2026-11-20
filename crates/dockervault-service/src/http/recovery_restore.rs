use super::{HttpState, auth::session_cookie, recovery_preview as recovery, security};
use crate::{
    auth::SessionToken,
    recovery_activation::{ClaimKind, JournalError, PreparationError},
    recovery_import::insert_state,
    storage_recovery::load_delete_cipher,
};
use axum::{
    Json,
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use dockervault_core::{
    portable_records::TABLE_ORDER, recovery_bundle::passwords_match, timestamp::Timestamp,
};
use recovery::{BUSY, Failure, INTERNAL, INVALID, REQUEST};
use serde::Deserialize;
use serde_json::{Value, json};
use std::{
    fs::{File, OpenOptions},
    os::unix::fs::OpenOptionsExt,
    path::Path,
};
use time::{SignedDuration, UtcDateTime, format_description::well_known::Rfc3339};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

const ACTIVATION: Failure = (
    StatusCode::INTERNAL_SERVER_ERROR,
    "System recovery activation failed; check setup status before retrying",
);
const CONFIGURED: Failure = (
    StatusCode::FORBIDDEN,
    "Recovery is only available before initial setup",
);
const KEY_FILE: &str = "/app/data/.credential_key";

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Restore {
    bundle: String,
    password: String,
    username: String,
    admin_password: String,
    confirm_admin_password: String,
}

pub(super) async fn restore(State(state): State<HttpState>, request: Request) -> Response {
    if let Err(err) = recovery::require_unconfigured(&state).await {
        return recovery::error(err);
    }
    if let Err(err) = security::rate_limit(&state, &request, "recovery-restore") {
        return *err;
    }
    let context = state.clone();
    let Ok(work) = state
        .probe_jobs
        .submit_async(move |cancel| run(context, request, cancel))
    else {
        return recovery::error(BUSY);
    };
    match work.wait().await.unwrap_or(Err(INTERNAL)) {
        Ok((body, token)) => {
            let mut response = Json(body).into_response();
            response
                .headers_mut()
                .insert(header::SET_COOKIE, session_cookie(&state, &token));
            response
        }
        Err(err) => recovery::error(err),
    }
}

async fn run(
    state: HttpState,
    request: Request,
    cancel: CancellationToken,
) -> Result<(Value, SessionToken), Failure> {
    let bytes = recovery::read_body(request, &cancel).await?;
    let Restore {
        bundle,
        password,
        username,
        admin_password,
        confirm_admin_password,
    } = serde_json::from_slice(&bytes).map_err(|_| REQUEST)?;
    drop(bytes);
    let confirmation = Zeroizing::new(confirm_admin_password);
    let admin_password = Zeroizing::new(admin_password);
    if bundle.len() > recovery::MAX_BUNDLE
        || [&password, &username, &*admin_password, &*confirmation]
            .iter()
            .any(|text| text.len() > 1024)
    {
        return Err(REQUEST);
    }
    if !passwords_match(&admin_password, &confirmation) {
        return Err((
            StatusCode::BAD_REQUEST,
            "Administrator passwords do not match",
        ));
    }
    let imported = recovery::validate_bundle(&state, bundle, password, &cancel).await?;
    if !(3..=255).contains(&username.chars().count())
        || !(8..=128).contains(&admin_password.chars().count())
    {
        return Err((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Invalid administrator account",
        ));
    }
    // First bounded activation slice: only storages whose observation is local.
    if imported.preview()["storages"]
        .as_array()
        .ok_or(INVALID)?
        .iter()
        .any(|row| row["type"] != "local")
    {
        return Err(INVALID);
    }
    let (hash, token) = state
        .auth
        .credentials(admin_password.to_string(), None, cancel.clone())
        .await
        .map_err(|_| INTERNAL)?;
    let hash = hash.ok_or(INTERNAL)?;
    let claims = state.auth.preparation_claims().clone();
    let config = state.config.clone();
    state.auth.blocking().submit_blocking(move |physical| {
        if cancel.is_cancelled() || physical.is_cancelled() { return Err(INTERNAL); }
        let guard = claims.try_claim(ClaimKind::Setup).map_err(|err| match err {
            PreparationError::Busy => BUSY,
            _ => ACTIVATION,
        })?;
        let key_file = Path::new(KEY_FILE);
        guard.discard_incomplete(key_file).map_err(|_| ACTIVATION)?;
        let generation = guard.begin_recovery().map_err(|err| match err {
            JournalError::AlreadySetup => CONFIGURED,
            _ => ACTIVATION,
        })?;
        let result = (|| {
            generation.prepare_instance_key(key_file, !config.credential_encryption_key.expose().is_empty(), &cancel)?;
            let cipher = load_delete_cipher(key_file, config.credential_encryption_key.expose(), || cancel.is_cancelled() || physical.is_cancelled()).map_err(|_| JournalError::Io)?;
            prepare_empty_generation(&generation.directory()?)?;
            let backup_root = std::path::absolute(&config.backup_base_path).map_err(|_| JournalError::Io)?;
            #[derive(Deserialize)]
            struct References { ssh_public_keys: Value }
            let refs = serde_json::from_str::<References>(imported.payload_json().as_str()).map_err(|_| JournalError::InvalidJournal)?.ssh_public_keys;
            generation.commit_activation(|tx| {
                if cancel.is_cancelled() || physical.is_cancelled() { return Err(JournalError::Cancelled); }
                // Never replace unrelated settings or retained operational rows.
                // This initial path requires an empty portable catalog.
                for table in TABLE_ORDER.into_iter().filter(|table| *table != "app_settings") {
                    if tx.query_row(&format!("SELECT EXISTS(SELECT 1 FROM {table})"), [], |row| row.get::<_, bool>(0)).map_err(|_| JournalError::Io)? {
                        return Err(JournalError::InvalidJournal);
                    }
                }
                insert_state(tx, &imported, &backup_root, &cipher, &cancel).map_err(|_| JournalError::Io)?;
                let observed = local_result(tx, &imported.preview()["counts"], &refs)?;
                let now = UtcDateTime::now();
                let format = |now: UtcDateTime| -> Result<String, JournalError> {
                    let value = now.format(&Rfc3339).map_err(|_| JournalError::Io)?;
                    Ok(Timestamp::parse(&value).map_err(|_| JournalError::Io)?.sql())
                };
                let stamp = format(now)?;
                tx.execute("INSERT INTO users(username,password_hash,is_admin,created_at,updated_at) VALUES(?,?,1,?,?)", [&username, &hash, &stamp, &stamp]).map_err(|_| JournalError::Io)?;
                let id = tx.last_insert_rowid();
                tx.execute("INSERT INTO sessions(user_id,token,created_at,expires_at) VALUES(?,?,?,?)", rusqlite::params![id, token.hash().as_str(), stamp, format(now + SignedDuration::days(7))?]).map_err(|_| JournalError::Io)?;
                for (key, value) in [("recovery_ssh_public_keys", refs.to_string()), ("recovery_result", observed.to_string())] {
                    tx.execute("INSERT INTO app_settings(key,value) VALUES(?,?)", [key, &value]).map_err(|_| JournalError::Io)?;
                }
                if cancel.is_cancelled() || physical.is_cancelled() { return Err(JournalError::Cancelled); }
                let created = now.format(&Rfc3339).map_err(|_| JournalError::Io)?;
                Ok(json!({"user":{"id":id,"username":username,"is_admin":true,"created_at":created,"last_login":null},"result":observed}))
            })
        })();
        if result.is_err() { guard.discard_incomplete(key_file).map_err(|_| ACTIVATION)?; }
        result.map(|body| (body, token)).map_err(|_| ACTIVATION)
    }).map_err(|_| BUSY)?.wait().await.map_err(|_| INTERNAL)?
}

fn prepare_empty_generation(path: &Path) -> Result<(), JournalError> {
    use nix::{
        fcntl::{OFlag, openat},
        sys::stat::{Mode, mkdirat},
    };
    let root = path.parent().ok_or(JournalError::Io)?;
    let data = OpenOptions::new()
        .read(true)
        .custom_flags(nix::libc::O_DIRECTORY | nix::libc::O_NOFOLLOW)
        .open(root.parent().ok_or(JournalError::Io)?)
        .map_err(|_| JournalError::Io)?;
    let flags = OFlag::O_DIRECTORY | OFlag::O_NOFOLLOW | OFlag::O_CLOEXEC;
    let leaf = Path::new(root.file_name().ok_or(JournalError::Io)?);
    match mkdirat(&data, leaf, Mode::S_IRWXU) {
        Ok(()) | Err(nix::errno::Errno::EEXIST) => (),
        Err(_) => return Err(JournalError::Io),
    }
    let root = openat(&data, leaf, flags, Mode::empty())
        .map(File::from)
        .map_err(|_| JournalError::Io)?;
    let leaf = Path::new(path.file_name().ok_or(JournalError::Io)?);
    mkdirat(&root, leaf, Mode::S_IRWXU).map_err(|_| JournalError::Io)?;
    openat(&root, leaf, flags, Mode::empty())
        .map(File::from)
        .map_err(|_| JournalError::Io)?
        .sync_all()
        .map_err(|_| JournalError::Io)?;
    root.sync_all()
        .and_then(|_| data.sync_all())
        .map_err(|_| JournalError::Io)
}

fn local_result(
    tx: &rusqlite::Transaction<'_>,
    counts: &Value,
    ssh: &Value,
) -> Result<Value, JournalError> {
    use std::collections::BTreeMap;
    let result = (|| -> rusqlite::Result<Value> {
        let mut bases = BTreeMap::new();
        let mut storages = Vec::new();
        let mut query = tx.prepare("SELECT id,base_path FROM remote_storages ORDER BY id")?;
        let mut rows = query.query([])?;
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let base: Option<String> = row.get(1)?;
            let base = base.filter(|path| std::fs::read_dir(path).is_ok());
            storages.push(json!({"id":id,"type":"local","state": if base.is_some() {"connected"} else {"offline"}}));
            bases.insert(id, base);
        }
        let mut backups = Vec::new();
        let mut query = tx.prepare(
            "SELECT id,file_path,encrypted,encryption_key_path FROM backups ORDER BY id",
        )?;
        let mut rows = query.query([])?;
        let mut copies_query = tx.prepare(
            "SELECT storage_id,remote_path FROM backup_storage_syncs WHERE backup_id=? ORDER BY id",
        )?;
        let file = |path: &Path| std::fs::symlink_metadata(path).is_ok_and(|info| info.is_file());
        while let Some(row) = rows.next()? {
            let id: i64 = row.get(0)?;
            let path: Option<String> = row.get(1)?;
            let key: Option<String> = row.get(3)?;
            let local = if !path.as_deref().is_some_and(|path| file(Path::new(path))) {
                "unavailable"
            } else if row.get::<_, bool>(2)?
                && !key.as_deref().is_some_and(|path| file(Path::new(path)))
            {
                "credentials_required"
            } else {
                "available"
            };
            let mut copies = Vec::new();
            let mut related = copies_query.query([id])?;
            while let Some(row) = related.next()? {
                let storage: i64 = row.get(0)?;
                let relative: Option<String> = row.get(1)?;
                let state = match bases.get(&storage).and_then(Option::as_ref) {
                    None => "offline",
                    Some(base)
                        if relative
                            .as_ref()
                            .is_some_and(|relative| file(&Path::new(base).join(relative))) =>
                    {
                        "available"
                    }
                    _ => "unavailable",
                };
                copies.push(json!({"storage_id":storage,"state":state}));
            }
            backups.push(json!({"id":id,"local":local,"copies":copies}));
        }
        Ok(
            json!({"storages":storages,"catalog":backups,"ssh_public_keys":ssh,"counts":counts,"archives_modified":false}),
        )
    })();
    result.map_err(|_| JournalError::Io)
}
