use super::{HttpState, security};
use crate::{
    auth::SessionHash,
    recovery_activation::{ClaimKind, PreparationError},
    recovery_records::snapshot_records,
    storage_recovery::load_delete_cipher,
};
use axum::{
    Json,
    extract::{Request, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use dockervault_core::{
    lossless_json::PythonJson,
    recovery_bundle::{BundleError, encode_json_bundle},
    recovery_state::state_manifest_json,
};
use futures_util::StreamExt;
use rusqlite::{Connection, OpenFlags, OptionalExtension};
use serde::Deserialize;
use serde_json::{json, value::to_raw_value};
use std::{path::Path, sync::Arc};
use time::{UtcDateTime, format_description::well_known::Rfc3339};
use tokio_util::sync::CancellationToken;
use zeroize::Zeroizing;

type Failure = (StatusCode, &'static str);
const BAD: Failure = (StatusCode::BAD_REQUEST, "Invalid recovery request");
const INVALID: Failure = (
    StatusCode::BAD_REQUEST,
    "Invalid or incompatible recovery bundle or password",
);
const BUSY: Failure = (
    StatusCode::CONFLICT,
    "Setup or recovery is already in progress",
);
const INTERNAL: Failure = (
    StatusCode::INTERNAL_SERVER_ERROR,
    "System recovery failed; inspect setup status before retrying",
);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Export {
    password: String,
    confirm_password: String,
}

fn error((status, detail): Failure) -> Response {
    (status, Json(json!({"detail":detail}))).into_response()
}

pub(super) async fn export(State(state): State<HttpState>, request: Request) -> Response {
    let token =
        security::cookie(request.headers()).or_else(|| security::bearer(request.headers(), true));
    match token {
        Some(token) => match state.auth.current(&SessionHash::from_token(&token)).await {
            Ok(Some(user)) if user.is_admin => (),
            Ok(Some(_)) => return error((StatusCode::FORBIDDEN, "Administrator access required")),
            Ok(None) => return error((StatusCode::UNAUTHORIZED, "Invalid or expired session")),
            Err(err) => return super::auth::error(err),
        },
        None => return error((StatusCode::UNAUTHORIZED, "Not authenticated")),
    }
    if let Err(response) = security::rate_limit(&state, &request, "recovery-export") {
        return *response;
    }
    let context = state.clone();
    let result = match state
        .probe_jobs
        .submit_async(move |cancel| run(context, request, cancel))
    {
        Ok(work) => work.wait().await.unwrap_or(Err(INTERNAL)),
        Err(_) => Err(BUSY),
    };
    match result {
        Ok(bundle) => (
            [
                (header::CONTENT_TYPE, "application/octet-stream"),
                (
                    header::CONTENT_DISPOSITION,
                    "attachment; filename=\"dockervault-system-backup.dockervault\"",
                ),
                (header::X_CONTENT_TYPE_OPTIONS, "nosniff"),
            ],
            bundle,
        )
            .into_response(),
        Err(failure) => error(failure),
    }
}

async fn run(
    state: HttpState,
    request: Request,
    cancel: CancellationToken,
) -> Result<Vec<u8>, Failure> {
    let mut stream = request.into_body().into_data_stream();
    let mut bytes = Zeroizing::new(Vec::new());
    loop {
        let chunk = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(INTERNAL),
            item = stream.next() => item,
        };
        let Some(chunk) = chunk else { break };
        let chunk = chunk.map_err(|_| BAD)?;
        if chunk.len() > (32 * 1024 * 1024 + 65_536) - bytes.len() {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                "Recovery request exceeds size limit",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    let Export {
        password,
        confirm_password,
    } = serde_json::from_slice(&bytes).map_err(|_| BAD)?;
    let password = Zeroizing::new(password);
    let confirm_password = Zeroizing::new(confirm_password);
    if password.len() > 1024 || confirm_password.len() > 1024 {
        return Err(BAD);
    }
    if password.chars().count() < 8
        || !dockervault_core::recovery_bundle::passwords_match(&password, &confirm_password)
    {
        return Err((
            StatusCode::BAD_REQUEST,
            "Enter and confirm the dedicated system-backup password",
        ));
    }
    let claims = state.auth.preparation_claims().clone();
    let path = state
        .auth
        .pool()
        .connect_options()
        .get_filename()
        .to_owned();
    let config = state.config.clone();
    let blocking = state.auth.blocking();
    let owner_cancel = cancel.clone();
    let (claim, mut catalog, ssh, managed, cipher) = blocking.submit_blocking(move |physical| {
        let cancelled = || owner_cancel.is_cancelled() || physical.is_cancelled();
        if cancelled() { return Err(INTERNAL); }
        let claim = claims.try_claim(ClaimKind::Crypto).map_err(|err| match err {
            PreparationError::Busy => BUSY,
            _ => INTERNAL,
        })?;
        let mut db = Connection::open_with_flags(path, OpenFlags::SQLITE_OPEN_READ_ONLY).map_err(|_| INTERNAL)?;
        let tx = db.transaction().map_err(|_| INTERNAL)?;
        let mut catalog = snapshot_records(&tx, &owner_cancel).map_err(|_| INVALID)?;
        let setting = |key, limit| -> Result<Option<String>, Failure> {
            let row: Option<(i64, Option<String>)> = tx.query_row(
                "SELECT coalesce(length(CAST(value AS BLOB)),0), CASE WHEN length(CAST(value AS BLOB)) <= ?2 THEN value END FROM app_settings WHERE key=?1",
                rusqlite::params![key, limit], |row| Ok((row.get(0)?, row.get(1)?))
            ).optional().map_err(|_| INTERNAL)?;
            match row {
                Some((length, _)) if length > limit => Err(INVALID),
                Some((_, value)) => Ok(value),
                None => Ok(None),
            }
        };
        let public = setting("recovery_ssh_public_keys", 2_i64 * 1024 * 1024)?;
        let has_rclone = catalog.remote_storages.iter().any(|row| row.storage_type == "rclone");
        let managed = if has_rclone {
            setting("recovery_rclone", 8_i64 * 1024 * 1024)?
                .filter(|raw| !raw.is_empty())
                .map(|raw| serde_json::from_str::<serde_json::Value>(&Zeroizing::new(raw)).map_err(|_| INVALID))
                .transpose()?
        } else {
            None
        };
        tx.commit().map_err(|_| INTERNAL)?;
        let needs_cipher = managed.is_some() || catalog.remote_storages.iter().any(|row| [&row.password, &row.s3_access_key, &row.s3_secret_key].into_iter().flatten().any(|secret| secret.starts_with("fernet:")));
        let cipher = if needs_cipher {
            let cipher = load_delete_cipher(Path::new("/app/data/.credential_key"), config.credential_encryption_key.expose(), cancelled).map_err(|_| INVALID)?;
            for row in &mut catalog.remote_storages {
                for secret in [&mut row.password, &mut row.s3_access_key, &mut row.s3_secret_key].into_iter().flatten() {
                    *secret = cipher.decrypt_value(secret).map_err(|_| INVALID)?;
                }
            }
            Some(Arc::new(cipher))
        } else {
            None
        };
        let ssh = super::ssh_keys::export_public(public.as_deref(), &owner_cancel).map_err(|_| INVALID)?;
        Ok((claim, catalog, ssh, managed, cipher))
    }).map_err(|_| BUSY)?.wait().await.map_err(|_| INTERNAL)??;
    let mut managed_packet = None;
    let mut external_rows = Vec::new();
    if let Some(managed) = managed {
        let roots = managed["roots"].as_array().ok_or(INVALID)?;
        let mut storages = Vec::new();
        for row in catalog
            .remote_storages
            .iter()
            .filter(|row| row.storage_type == "rclone")
        {
            let mut matching = roots
                .iter()
                .filter(|root| root["storage_id"].as_i64() == Some(row.id));
            let Some(root) = matching.next() else {
                external_rows.push(row.clone());
                continue;
            };
            if matching.next().is_some()
                || root["remote_name"].as_str() != row.rclone_remote.as_deref()
            {
                return Err(INVALID);
            }
            storages.push(row.clone());
        }
        let binary = Path::new("/usr/local/bin/rclone");
        let processes = state.probe_processes.as_ref().ok_or(INVALID)?;
        let options = crate::rclone::provider_options(processes, blocking, binary, cancel.clone())
            .await
            .map_err(|_| INVALID)?;
        let runner = crate::rclone::ManagedConfigBuilder::new(
            processes.clone(),
            blocking.clone(),
            state.probe_jobs.clone(),
            binary.into(),
        );
        let work = runner
            .start_export_managed(managed, cipher.ok_or(INVALID)?, storages, options.clone())
            .map_err(|_| BUSY)?;
        let stop = work.cancellation_token();
        let waiting = work.wait();
        tokio::pin!(waiting);
        let packet = tokio::select! {
            biased;
            _ = cancel.cancelled() => {
                stop.cancel();
                let _ = waiting.await;
                return Err(INTERNAL);
            }
            outcome = &mut waiting => outcome.map_err(|_| INTERNAL)?.map_err(|_| INVALID)?,
        };
        managed_packet = Some((packet, options));
    } else {
        external_rows.extend(
            catalog
                .remote_storages
                .iter()
                .filter(|row| row.storage_type == "rclone")
                .cloned(),
        );
    }
    let external = if !external_rows.is_empty() {
        use std::os::unix::fs::OpenOptionsExt;
        let path = state.config.rclone_config.clone();
        let file = blocking
            .submit_blocking(move |_| {
                match std::fs::OpenOptions::new()
                    .read(true)
                    .custom_flags(nix::libc::O_NOFOLLOW | nix::libc::O_NONBLOCK)
                    .open(path)
                {
                    Ok(file) => {
                        let metadata = file.metadata().map_err(|_| INVALID)?;
                        if !metadata.is_file() || metadata.len() > 8 * 1024 * 1024 {
                            return Err(INVALID);
                        }
                        Ok(Some(file))
                    }
                    Err(err)
                        if matches!(
                            err.kind(),
                            std::io::ErrorKind::NotFound | std::io::ErrorKind::PermissionDenied
                        ) =>
                    {
                        Ok(None)
                    }
                    Err(_) => Err(INVALID),
                }
            })
            .map_err(|_| BUSY)?
            .wait()
            .await
            .map_err(|_| INTERNAL)??;
        let (mut remotes, options) = if let Some(file) = file {
            let processes = state.probe_processes.as_ref().ok_or(INVALID)?;
            let binary = Path::new("/usr/local/bin/rclone");
            let remotes = crate::rclone::external_config_dump(
                processes,
                file,
                state.config.rclone_config_pass.expose(),
                &cancel,
            )
            .await
            .map_err(|_| INVALID)?;
            let options =
                crate::rclone::provider_options(processes, blocking, binary, cancel.clone())
                    .await
                    .map_err(|_| INVALID)?;
            (remotes, options)
        } else {
            (json!({}), Default::default())
        };
        dockervault_core::rclone_package::normalize_external_paths(&mut remotes, &|| {
            cancel.is_cancelled()
        })
        .map_err(|_| INVALID)?;
        let empty = json!({});
        let package = dockervault_core::rclone_package::assemble_canonical(
            &remotes,
            &empty,
            &external_rows,
            &json!([]),
            &options,
            &|| cancel.is_cancelled(),
        )
        .map_err(|_| INVALID)?;
        // A present but non-canonical referenced remote still needs the external
        // normalization adapter. Do not conceal it as a missing configuration.
        if package["roots"]
            .as_array()
            .ok_or(INVALID)?
            .iter()
            .any(|root| {
                root["reason"] == "external_or_unsafe_dependency"
                    && root["remote_name"]
                        .as_str()
                        .is_some_and(|name| remotes.get(name).is_some())
            })
        {
            return Err(INVALID);
        }
        dockervault_core::rclone_package::validate(&package, &external_rows, &options, &|| {
            cancel.is_cancelled()
        })
        .map_err(|_| INVALID)?;
        Some(package)
    } else {
        None
    };
    let rclone = match (managed_packet, external) {
        (Some((managed, options)), Some(external)) => {
            dockervault_core::rclone_package::merge_snapshots(
                managed,
                external,
                &mut catalog.remote_storages,
                &options,
                &|| cancel.is_cancelled(),
            )
            .map_err(|_| INVALID)?
        }
        (Some((packet, _)), None) | (None, Some(packet)) => packet,
        (None, None) => json!({"schema_version":1,"roots":[],"remotes":{},"files":{}}),
    };
    let root = std::path::absolute(&state.config.backup_base_path).map_err(|_| INTERNAL)?;
    blocking.submit_blocking(move |physical| {
        let _claim = claim;
        let cancelled = || cancel.is_cancelled() || physical.is_cancelled();
        let export_time = UtcDateTime::now().format(&Rfc3339).map_err(|_| INTERNAL)?;

        let header = json!({"schema_version":1,"application":"dockervault","compatibility_version":1,
            "exported_at": export_time, "backup_root":root.to_str().ok_or(INTERNAL)?, "ssh_public_keys":ssh,
            "rclone":rclone});
        let mut fields = header.as_object().ok_or(INTERNAL)?.iter().map(|(name, value)| {
            Ok((name.as_str(), to_raw_value(value).map_err(|_| INTERNAL)?))
        }).collect::<Result<std::collections::BTreeMap<_,_>, Failure>>()?;
        fields.insert("tables", to_raw_value(&catalog).map_err(|_| INTERNAL)?);
        let json = |fields: &std::collections::BTreeMap<&str, Box<serde_json::value::RawValue>>| {
            PythonJson::parse(&serde_json::to_vec(fields).map_err(|_| INTERNAL)?).map_err(|_| INVALID)
        };
        let manifest = state_manifest_json(&json(&fields)?, &cancelled).map_err(|_| INVALID)?;
        fields.insert("manifest", to_raw_value(&manifest).map_err(|_| INTERNAL)?);
        if cancelled() { return Err(INTERNAL); }
        encode_json_bundle(json(&fields)?, &password).map_err(|err| match err {
            BundleError::Invalid => INVALID,
            BundleError::Busy => BUSY,
            BundleError::Internal => INTERNAL,
        })
    }).map_err(|_| BUSY)?.wait().await.map_err(|_| INTERNAL)?
}
