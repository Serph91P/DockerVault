use super::{HttpState, auth::required_user};
use crate::{
    jobs::Resource,
    journal::{Journal, Kind, RESTORE_WRITE_PHASE},
    restore::IndexedArchive,
    storage::{hash_reader, local},
};
use axum::{
    Json,
    extract::{Path as RoutePath, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use dockervault_core::encoding::python_json;
use serde::Deserialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use sqlx::{Row, sqlite::SqliteRow};
use std::{
    fs::File,
    io::{self, Seek},
    os::unix::fs::MetadataExt,
    path::{Path, PathBuf},
};
use tokio_util::sync::CancellationToken;

type Failure = (StatusCode, &'static str);
const INTERNAL: Failure = (StatusCode::INTERNAL_SERVER_ERROR, "Restore failed");
const STALE: Failure = (StatusCode::CONFLICT, "Restore impact changed; review again");
const UNAVAILABLE: Failure = (
    StatusCode::NOT_FOUND,
    "Backup archive is not available locally or remotely",
);
const UNSUPPORTED: Failure = (
    StatusCode::SERVICE_UNAVAILABLE,
    "This restore configuration is not yet supported by the native candidate",
);
fn error((status, message): Failure) -> Response {
    (status, Json(json!({"detail":message}))).into_response()
}

#[derive(Deserialize)]
pub(super) struct Selection {
    destination_type: Option<String>,
    target_path: Option<String>,
    target_volume: Option<String>,
    review_fingerprint: Option<String>,
}

struct Impact {
    target: i64,
    destination: PathBuf,
    identity: (u64, u64),
    archive_path: Option<String>,
    checksum: Option<String>,
    length: Option<i64>,
    layout: Option<String>,
    public: Value,
}

pub(super) async fn review(
    State(state): State<HttpState>,
    RoutePath(id): RoutePath<i64>,
    headers: HeaderMap,
    input: Result<Json<Selection>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if let Err(e) = required_user(&state, &headers).await {
        return *e;
    }
    let Ok(Json(selection)) = input else {
        return error((
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid restore destination",
        ));
    };
    match impact(&state, id, &selection).await {
        Ok(value) => Json(value.public).into_response(),
        Err(e) => error(e),
    }
}

pub(super) async fn restore(
    State(state): State<HttpState>,
    RoutePath(id): RoutePath<i64>,
    headers: HeaderMap,
    input: Result<Json<Selection>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if let Err(e) = required_user(&state, &headers).await {
        return *e;
    }
    let Ok(Json(selection)) = input else {
        return error((
            StatusCode::UNPROCESSABLE_ENTITY,
            "Invalid restore destination",
        ));
    };
    if selection.destination_type.is_some() && selection.review_fingerprint.is_none() {
        return error((
            StatusCode::CONFLICT,
            "Review the destination before confirmation",
        ));
    }
    let worker = state.clone();
    let Ok(work) = state
        .backup_jobs
        .submit_async(move |cancel| async move { execute(&worker, id, &selection, &cancel).await })
    else {
        return error((
            StatusCode::SERVICE_UNAVAILABLE,
            "Restore capacity exhausted",
        ));
    };
    match work.wait().await {
        Ok(Ok(mut value)) => {
            value["status"] = json!("restored");
            Json(value).into_response()
        }
        Ok(Err(failure)) => error(failure),
        Err(_) => error(INTERNAL),
    }
}

async fn impact(state: &HttpState, id: i64, selection: &Selection) -> Result<Impact, Failure> {
    if !matches!(
        selection.destination_type.as_deref(),
        None | Some("original")
    ) || selection.target_path.is_some()
        || selection.target_volume.is_some()
    {
        return Err(UNSUPPORTED);
    }
    let row = sqlx::query("SELECT b.status,b.target_id,b.file_path,b.file_size,b.checksum,b.encrypted,b.encryption_key_path,b.backup_metadata,
        t.id AS plan_id,t.target_type,t.host_path,t.volume_name,t.container_name,t.stop_container,t.dependencies
        FROM backups b LEFT JOIN backup_targets t ON t.id=b.target_id WHERE b.id=?")
        .bind(id).fetch_optional(state.auth.pool()).await.map_err(|_| INTERNAL)?
        .ok_or((StatusCode::NOT_FOUND,"Backup not found"))?;
    if field::<Option<String>>(&row, "status")?.as_deref() != Some("COMPLETED") {
        return Err((
            StatusCode::BAD_REQUEST,
            "Only completed backups can be restored",
        ));
    }
    let target: i64 = field::<Option<i64>>(&row, "plan_id")?
        .ok_or((StatusCode::BAD_REQUEST, "Source plan is unavailable"))?;
    let encrypted = field::<Option<bool>>(&row, "encrypted")?.unwrap_or(false);
    let host = field::<Option<String>>(&row, "host_path")?;
    let container = field::<Option<String>>(&row, "container_name")?;
    let stop = field::<Option<bool>>(&row, "stop_container")?;
    let dependencies = field::<Option<String>>(&row, "dependencies")?
        .map(|s| serde_json::from_str::<Value>(&s))
        .transpose()
        .map_err(|_| INTERNAL)?
        .unwrap_or(Value::Null);
    if encrypted
        || field::<String>(&row, "target_type")? != "path"
        || (stop == Some(true) && container.as_deref().is_some_and(|s| !s.is_empty()))
        || !(dependencies.is_null() || dependencies.as_array().is_some_and(Vec::is_empty))
    {
        return Err(UNSUPPORTED);
    }
    let original = PathBuf::from(
        host.as_deref()
            .filter(|s| !s.is_empty())
            .ok_or(UNSUPPORTED)?,
    );
    let backup_root = PathBuf::from(&state.config.backup_base_path);
    let (destination, identity) = state
        .auth
        .blocking()
        .submit_blocking(move |_| {
            let canonical = std::fs::canonicalize(&original)?;
            let backups = std::fs::canonicalize(backup_root)?;
            if canonical.starts_with(&backups) || backups.starts_with(&canonical) {
                return Err(io::Error::other("Destination overlaps archive storage"));
            }
            let root = local::open_root(&canonical)?;
            Ok::<_, io::Error>((canonical, dir_identity(&root)?))
        })
        .map_err(|_| INTERNAL)?
        .wait()
        .await
        .map_err(|_| INTERNAL)?
        .map_err(|_| (StatusCode::BAD_REQUEST, "Destination is unavailable"))?;
    let metadata = field::<Option<String>>(&row, "backup_metadata")?
        .map(|s| serde_json::from_str::<Value>(&s))
        .transpose()
        .map_err(|_| INTERNAL)?
        .unwrap_or(Value::Null);
    let layout = metadata
        .get("archive_layout")
        .cloned()
        .unwrap_or_else(|| json!({}));
    let strip = match layout.get("kind").and_then(Value::as_str) {
        Some("single") => Some(
            layout
                .get("root")
                .and_then(Value::as_str)
                .unwrap_or_default()
                .to_owned(),
        ),
        None if layout == json!({}) => Some(String::new()),
        _ => return Err(UNSUPPORTED),
    };
    let archive_path: Option<String> = field(&row, "file_path")?;
    let checksum: Option<String> = field(&row, "checksum")?;
    let length = field(&row, "file_size")?;
    let mut public = json!({"destination":{"type":"original","path":destination,"volume":null},"containers_to_stop":[],"containers_to_restart":[],"layout":"Contents of the single archive root merge into the destination","overwrite":"Colliding files are overwritten; unrelated files remain. No transactional rollback or application-health guarantee.","key_required":false});
    let mut signed = public.clone();
    signed["archive"] = json!({"id":id,"target_id":target,"file_path":archive_path,"checksum":checksum,"encrypted":false,"key_path":field::<Option<String>>(&row,"encryption_key_path")?,"layout":layout,"status":"completed"});
    signed["target"] = json!({"id":target,"target_type":"path","host_path":host,"volume_name":field::<Option<String>>(&row,"volume_name")?,"container_name":container,"stop_container":stop,"dependencies":dependencies});
    signed["destination_identity"] =
        json!({"path":destination,"ancestor":destination,"device":identity.0,"inode":identity.1});
    signed["volume_identity"] = Value::Null;
    signed["containers"] = json!([]);
    signed.sort_all_objects();
    public["fingerprint"] = json!(hex(&Sha256::digest(python_json(&signed).as_bytes())));
    Ok(Impact {
        target,
        destination,
        identity,
        archive_path,
        checksum,
        length,
        layout: strip,
        public,
    })
}

async fn execute(
    state: &HttpState,
    id: i64,
    selection: &Selection,
    cancel: &CancellationToken,
) -> Result<Value, Failure> {
    let initial = impact(state, id, selection).await?;
    let _target = state
        .backup_claims
        .acquire([Resource::target(initial.target)], cancel)
        .await
        .map_err(|_| INTERNAL)?;
    let path = initial.destination.clone();
    let claim = state
        .auth
        .blocking()
        .submit_blocking(move |_| Resource::existing_path(&path))
        .map_err(|_| INTERNAL)?
        .wait()
        .await
        .map_err(|_| INTERNAL)?
        .map_err(|_| STALE)?;
    let _path = state
        .backup_claims
        .acquire([claim], cancel)
        .await
        .map_err(|_| INTERNAL)?;
    let reviewed = impact(state, id, selection).await?;
    if reviewed.public["fingerprint"] != initial.public["fingerprint"]
        || selection
            .review_fingerprint
            .as_deref()
            .is_some_and(|f| reviewed.public["fingerprint"] != f)
    {
        return Err(STALE);
    }
    crate::recovery_activation::require_recovery_ready(state.auth.pool())
        .await
        .map_err(|_| UNSUPPORTED)?;
    let journal = Journal::new(state.auth.pool().clone()).map_err(|_| INTERNAL)?;
    let unresolved: bool = sqlx::query_scalar(
        "SELECT EXISTS(SELECT 1 FROM workload_operations WHERE state='recovery_required')",
    )
    .fetch_one(state.auth.pool())
    .await
    .map_err(|_| INTERNAL)?;
    if unresolved {
        return Err((
            StatusCode::CONFLICT,
            "Workload recovery incomplete; retry reconciliation before new jobs",
        ));
    }
    let key = journal
        .begin(Some(id), Kind::Restore, indexmap::IndexMap::new(), None)
        .await
        .map_err(|_| INTERNAL)?;
    let token = cancel.clone();
    let root = PathBuf::from(&state.config.backup_base_path);
    let writer_journal = journal.clone();
    let runtime = tokio::runtime::Handle::current();
    let worker = state.auth.blocking().submit_blocking(move |owner_cancel| {
        let cancelled = || token.is_cancelled() || owner_cancel.is_cancelled();
        restore_path(&reviewed, &root, &cancelled, || {
            runtime
                .block_on(writer_journal.phase(key, RESTORE_WRITE_PHASE))
                .map_err(io::Error::other)
        })?;
        Ok::<_, Failure>(reviewed.public)
    });
    let result = match worker {
        Ok(work) => work.wait().await.unwrap_or(Err(INTERNAL)),
        Err(_) => Err(INTERNAL),
    };
    journal
        .settle_native_restore(key, result.is_ok())
        .await
        .map_err(|_| INTERNAL)?;
    result
}

fn restore_path(
    impact: &Impact,
    root: &Path,
    cancel: &dyn Fn() -> bool,
    mark_intent: impl FnOnce() -> io::Result<()>,
) -> Result<(), Failure> {
    let root = std::path::absolute(root).map_err(|_| INTERNAL)?;
    let archive = impact.archive_path.as_ref().ok_or(UNAVAILABLE)?;
    let archive = std::path::absolute(archive).map_err(|_| UNAVAILABLE)?;
    let relative = archive.strip_prefix(&root).map_err(|_| UNAVAILABLE)?;
    let input = local::open_download_source(&root, relative).map_err(|_| UNAVAILABLE)?;
    let (mut snapshot, length) =
        local::snapshot_download_source(input, cancel).map_err(|_| INTERNAL)?;
    if impact
        .length
        .is_some_and(|len| u64::try_from(len) != Ok(length))
    {
        return Err(INTERNAL);
    }
    if let Some(checksum) = &impact.checksum {
        let digest = hash_reader(&mut snapshot, cancel).map_err(|_| INTERNAL)?;
        if !checksum.eq_ignore_ascii_case(&hex(&digest)) {
            return Err(INTERNAL);
        }
        snapshot.rewind().map_err(|_| INTERNAL)?;
    }
    let mut index = IndexedArchive::prepare(
        snapshot,
        relative.extension().is_some_and(|ext| ext == "gz"),
        &local::open_root(&root).map_err(|_| INTERNAL)?,
        impact.layout.as_deref().map(str::as_bytes),
        cancel,
    )
    .map_err(|_| INTERNAL)?;
    let dest = local::open_root(&impact.destination).map_err(|_| STALE)?;
    if dir_identity(&dest).map_err(|_| STALE)? != impact.identity {
        return Err(STALE);
    }
    let mut stale = false;
    let result = index.restore_into_guarded(&dest, cancel, || {
        if local::open_root(&impact.destination)
            .and_then(|file| dir_identity(&file))
            .ok()
            != Some(impact.identity)
        {
            stale = true;
            return Err(io::Error::other("Restore destination changed"));
        }
        mark_intent()
    });
    result.map_err(|_| if stale { STALE } else { INTERNAL })
}

fn field<T>(row: &SqliteRow, key: &str) -> Result<T, Failure>
where
    for<'a> T: sqlx::Decode<'a, sqlx::Sqlite> + sqlx::Type<sqlx::Sqlite>,
{
    row.try_get(key).map_err(|_| INTERNAL)
}
fn dir_identity(file: &File) -> io::Result<(u64, u64)> {
    let meta = file.metadata()?;
    Ok((meta.dev(), meta.ino()))
}
fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|b| format!("{b:02x}")).collect()
}
