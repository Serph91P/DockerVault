#[path = "backup_archive_download.rs"]
mod download;
pub(super) use download::whole_archive as download;

use super::{
    HttpState,
    auth::{required_user, validation},
    operations::query_integer,
};
use axum::{
    Json,
    extract::{Path, Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use sqlx::{Execute, QueryBuilder, Row, Sqlite, SqliteConnection, sqlite::SqliteRow};
use std::{collections::HashMap, io::ErrorKind, path::Path as FsPath};
use strum::EnumMessage;
use time::{
    Date, OffsetDateTime, PrimitiveDateTime, UtcOffset, format_description::well_known::Rfc3339,
    macros::format_description,
};

const FROM: &str = " FROM backups b LEFT JOIN backup_targets t ON t.id=b.target_id WHERE 1=1";
const SELECT: &str = "SELECT b.*, t.name AS target_name, t.target_type, t.container_name, t.stack_name, t.volume_name, t.host_path, t.removed_at, t.retention_policy_id, t.remote_storage_ids, t.stop_container, t.dependencies,
    (SELECT step FROM backup_logs WHERE backup_id=b.id ORDER BY created_at DESC,id DESC LIMIT 1) AS stage";
const DATASET: &str = "Catalog records up to snapshot ID; location is recorded placement, not a live availability scan. Existing records may change; restart pagination to include new IDs.";
const INTEGER: &str = "Input should be a valid integer, unable to parse string as an integer";
const STATUSES: [&str; 5] = ["pending", "running", "completed", "failed", "cancelled"];
const SQL_TIME: &[time::format_description::BorrowedFormatItem<'_>] =
    format_description!("[year]-[month]-[day] [hour]:[minute]:[second].[subsecond digits:6]");

struct Filters {
    values: HashMap<String, String>,
    errors: Vec<Value>,
}
impl Filters {
    fn error(&mut self, field: &str, message: impl Into<String>) {
        self.errors
            .push(json!({"field":format!("query.{field}"),"message":message.into()}));
    }
    fn integer(&mut self, field: &str, min: Option<i64>, max: Option<i64>) -> Option<i64> {
        let raw = self.values.get(field)?;
        match query_integer(raw) {
            Some(value) if min.is_some_and(|min| value < min) => self.error(
                field,
                format!("Input should be greater than or equal to {}", min.unwrap()),
            ),
            Some(value) if max.is_some_and(|max| value > max) => self.error(
                field,
                format!("Input should be less than or equal to {}", max.unwrap()),
            ),
            Some(value) => return Some(value),
            None => self.error(field, INTEGER),
        }
        None
    }
    fn choice(&mut self, field: &str, options: &[&'static str]) -> Option<&'static str> {
        let value = self.values.get(field)?;
        if let Some(found) = options.iter().find(|&&option| option == value) {
            return Some(found);
        }
        let (last, earlier) = options.split_last().unwrap();
        let expected = earlier
            .iter()
            .map(|v| format!("'{v}'"))
            .collect::<Vec<_>>()
            .join(", ");
        self.error(field, format!("Input should be {expected} or '{last}'"));
        None
    }
    fn date(&mut self, field: &str) -> Option<String> {
        let raw = self.values.get(field)?;
        match history_boundary(raw) {
            Ok(value) => Some(value),
            Err(reason) => {
                self.error(
                    field,
                    format!("Input should be a valid datetime or date, {reason}"),
                );
                None
            }
        }
    }
}

fn history_boundary(raw: &str) -> Result<String, &'static str> {
    // Match the previous HTTP parser, including lax ISO and numeric timestamps.
    use speedate::{
        Date, DateTime, DateTimeConfig, MicrosecondsPrecisionOverflowBehavior, TimeConfig,
        TimestampUnit,
    };
    let config = DateTimeConfig {
        timestamp_unit: TimestampUnit::Infer,
        time_config: TimeConfig {
            microseconds_precision_overflow_behavior:
                MicrosecondsPrecisionOverflowBehavior::Truncate,
            unix_timestamp_offset: Some(0),
        },
    };
    let (seconds, micros) = match DateTime::parse_bytes_with_config(raw.as_bytes(), &config) {
        Ok(date) => (date.timestamp_tz(), date.time.microsecond),
        Err(_) => {
            let date = Date::parse_str(raw).map_err(|err| {
                err.get_documentation()
                    .unwrap_or("invalid date or datetime")
            })?;
            (date.timestamp(), 0)
        }
    };
    let instant = i128::from(seconds) * 1_000_000_000 + i128::from(micros) * 1000;
    OffsetDateTime::from_unix_timestamp_nanos(instant)
        .ok()
        .filter(|dt| (1..=9999).contains(&dt.year()))
        .and_then(|dt| dt.format(SQL_TIME).ok())
        .ok_or("date is outside the supported range")
}

fn date(value: &str) -> Option<OffsetDateTime> {
    let value = value.replacen(' ', "T", 1);
    OffsetDateTime::parse(&value, &Rfc3339)
        .or_else(|_| OffsetDateTime::parse(&format!("{value}Z"), &Rfc3339))
        .ok()
        .or_else(|| {
            Date::parse(&value, format_description!("[year]-[month]-[day]"))
                .ok()
                .map(|date| date.midnight().assume_utc())
        })
}
fn timestamp(row: &SqliteRow, col: &str) -> Result<Option<String>, sqlx::Error> {
    let raw: Option<String> = row.try_get(col)?;
    raw.map(|raw| {
        let parsed = date(&raw).ok_or(sqlx::Error::Protocol("invalid catalog timestamp".into()))?;
        let native = parsed.to_offset(UtcOffset::UTC);
        let clock = PrimitiveDateTime::new(native.date(), native.time());
        let mut out = clock
            .format(format_description!(
                "[year]-[month]-[day]T[hour]:[minute]:[second]"
            ))
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))?;
        if native.microsecond() != 0 {
            out.push_str(&format!(".{:06}", native.microsecond()));
        }
        out.push_str(
            if raw.ends_with('Z') || raw.get(19..).is_some_and(|tail| tail.contains(['-', '+'])) {
                "+00:00"
            } else {
                "Z"
            },
        );
        Ok(out)
    })
    .transpose()
}
fn detail(status: StatusCode, message: &str) -> Response {
    (status, Json(json!({"detail":message}))).into_response()
}
fn internal() -> Response {
    detail(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
}

pub(super) async fn list(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(values): Query<HashMap<String, String>>,
) -> Response {
    if let Err(error) = required_user(&state, &headers).await {
        return *error;
    }
    let mut f = Filters {
        values,
        errors: vec![],
    };
    let target = f.integer("target_id", None, None);
    let status = f.choice("status", &STATUSES);
    let limit = f.integer("limit", Some(1), Some(1000)).unwrap_or(50);
    let offset = f.integer("offset", Some(0), None).unwrap_or(0);
    if !f.errors.is_empty() {
        return *validation(f.errors);
    }
    let mut query = QueryBuilder::new(format!("{SELECT}{FROM}"));
    if let Some(target) = target.filter(|&target| target != 0) {
        query.push(" AND b.target_id=").push_bind(target);
    }
    if let Some(status) = status {
        query
            .push(" AND b.status=")
            .push_bind(status.to_ascii_uppercase());
    }
    query
        .push(" ORDER BY b.created_at DESC LIMIT ")
        .push_bind(limit)
        .push(" OFFSET ")
        .push_bind(offset);
    match page_rows(&state, query).await {
        Ok(rows) => Json(rows).into_response(),
        Err(error) => *error,
    }
}

pub(super) async fn history(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(values): Query<HashMap<String, String>>,
) -> Response {
    if let Err(error) = required_user(&state, &headers).await {
        return *error;
    }
    let mut f = Filters {
        values,
        errors: vec![],
    };
    let view = f.choice("view", &["runs", "archives"]).unwrap_or("runs");
    let q = f.values.get("q").cloned().unwrap_or_default();
    if q.chars().count() > 200 {
        f.error("q", "String should have at most 200 characters");
    }
    let status = f.choice("status", &STATUSES);
    let source = f.choice("source", &["container", "stack", "volume", "path"]);
    let target = f.integer("target_id", Some(1), None);
    let after = f.date("after");
    let before = f.date("before");
    let location = f.values.get("location").cloned().unwrap_or_default();
    if !matches!(location.as_str(), "" | "local" | "remote")
        && (!location.starts_with(|c: char| matches!(c, '1'..='9'))
            || !location.bytes().all(|c| c.is_ascii_digit()))
    {
        f.error(
            "location",
            "String should match pattern '^(|local|remote|[1-9][0-9]*)$'",
        );
    }
    let sort = match f.choice("sort", &["created", "name", "size"]) {
        Some("name") => "t.name",
        Some("size") => "b.file_size",
        _ => "b.created_at",
    };
    let direction = if f.choice("direction", &["asc", "desc"]) == Some("asc") {
        "ASC"
    } else {
        "DESC"
    };
    let limit = f.integer("limit", Some(1), Some(1000)).unwrap_or(50);
    let offset = f.integer("offset", Some(0), None).unwrap_or(0);
    let snapshot = f.integer("snapshot", Some(0), None);
    if !f.errors.is_empty() {
        return *validation(f.errors);
    }
    let result = async {
        let mut db = state.auth.pool().begin().await?;
        let snapshot = match snapshot { Some(snapshot) => snapshot, None => sqlx::query_scalar::<_, i64>("SELECT coalesce(max(id),0) FROM backups").fetch_one(&mut *db).await? };
        let mut terms: QueryBuilder<Sqlite> = QueryBuilder::new(FROM);
        terms.push(" AND b.id<=").push_bind(snapshot);
        if view == "archives" { terms.push(" AND (b.file_path IS NOT NULL OR b.file_size IS NOT NULL OR EXISTS (SELECT 1 FROM backup_storage_syncs s WHERE s.backup_id=b.id AND s.sync_status='completed' AND s.remote_path IS NOT NULL))"); }
        if !q.trim().is_empty() {
            let needle = format!("%{}%", q.trim().replace('/', "//").replace('%', "/%").replace('_', "/_"));
            terms.push(" AND (");
            for (idx, column) in ["t.name", "t.container_name", "t.stack_name", "t.volume_name", "t.host_path", "b.file_path", "b.error_message", "CAST(b.id AS TEXT)"].into_iter().enumerate() {
                if idx > 0 { terms.push(" OR "); }
                terms.push("lower(").push(column).push(") LIKE lower(").push_bind(needle.clone()).push(") ESCAPE '/'");
            }
            terms.push(")");
        }
        if let Some(status) = status { terms.push(" AND b.status=").push_bind(status.to_ascii_uppercase()); }
        if let Some(source) = source { terms.push(" AND t.target_type=").push_bind(source); }
        if let Some(target) = target { terms.push(" AND b.target_id=").push_bind(target); }
        if let Some(after) = after { terms.push(" AND b.created_at>=").push_bind(after); }
        if let Some(before) = before { terms.push(" AND b.created_at<=").push_bind(before); }
        match location.as_str() {
            "" => {},
            "local" => { terms.push(" AND b.file_path IS NOT NULL"); },
            remote => {
                terms.push(" AND EXISTS (SELECT 1 FROM backup_storage_syncs s WHERE s.backup_id=b.id");
                if remote != "remote" { terms.push(" AND s.storage_id=").push_bind(remote.parse::<i64>().ok()); }
                terms.push(")");
            }
        }
        // Count and page share the same connection/read snapshot, including an empty page.
        let args = terms.build().take_arguments().map_err(|e| sqlx::Error::Protocol(e.to_string()))?.expect("query builder has arguments");
        let terms_sql = terms.into_string();
        let total = QueryBuilder::<Sqlite>::with_arguments(format!("SELECT count(*){terms_sql}"), args.clone()).build_query_scalar::<i64>().fetch_one(&mut *db).await?;
        let mut page = QueryBuilder::<Sqlite>::with_arguments(format!("{SELECT}{terms_sql} ORDER BY {sort} {direction},b.id {direction} LIMIT "), args);
        page.push_bind(limit).push(" OFFSET ").push_bind(offset);
        let rows = page.build().fetch_all(&mut *db).await?;
        let copies = copies(&mut db, &rows).await?;
        db.commit().await?;
        Ok::<_, sqlx::Error>((total, snapshot, rows, copies))
    }.await;
    let Ok((total, snapshot, rows, copies)) = result else {
        return internal();
    };
    match render(&state, rows, copies, true).await {
        Ok(items) => Json(json!({"items":items,"total":total,"snapshot":snapshot,"limit":limit,"offset":offset,"dataset":DATASET})).into_response(),
        Err(error) => *error,
    }
}

pub(super) async fn get(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    one(state, headers, id, false, false).await
}
pub(super) async fn history_detail(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    one(state, headers, id, true, false).await
}
pub(super) async fn restore_info(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    one(state, headers, id, false, true).await
}
async fn one(
    state: HttpState,
    headers: HeaderMap,
    id: String,
    history: bool,
    restore: bool,
) -> Response {
    if let Err(error) = required_user(&state, &headers).await {
        return *error;
    }
    let Some(id) = query_integer(&id) else {
        return *validation(vec![json!({"field":"path.backup_id","message":INTEGER})]);
    };
    let mut query = QueryBuilder::new(format!("{SELECT}{FROM} AND b.id="));
    query.push_bind(id);
    let result = async {
        let mut db = state.auth.pool().begin().await?;
        let rows = query.build().fetch_all(&mut *db).await?;
        let copies = copies(&mut db, &rows).await?;
        db.commit().await?;
        Ok::<_, sqlx::Error>((rows, copies))
    }
    .await;
    let Ok((rows, copies)) = result else {
        return internal();
    };
    let Some(row) = rows.first() else {
        return detail(
            StatusCode::NOT_FOUND,
            if history {
                "Run or archive not found"
            } else {
                "Backup not found"
            },
        );
    };
    if restore {
        if row.get::<Option<&str>, _>("status") != Some("COMPLETED") {
            return detail(
                StatusCode::BAD_REQUEST,
                "Only completed backups can be restored",
            );
        }
        let info = match restore_details(row) {
            Ok(info) => info,
            Err(_) => return internal(),
        };
        let (volumes, discovery) = if info["target"]["id"].is_null() {
            (vec![], "available")
        } else {
            let socket = state.config.docker_socket.clone();
            let Ok(work) = state.probe_jobs.submit_async(move |_| async move {
                let docker = crate::docker::DockerClient::connect(&socket).await?;
                let result = docker.volume_names().await;
                docker.shutdown(None).await;
                result
            }) else {
                return detail(
                    StatusCode::SERVICE_UNAVAILABLE,
                    "Catalog reader unavailable",
                );
            };
            match work.wait().await {
                Ok(Ok(volumes)) => (volumes, "available"),
                _ => (vec![], "unavailable"),
            }
        };
        let mut info = info;
        let mut entries = match render(&state, rows, copies, false).await {
            Ok(entries) => entries,
            Err(error) => return *error,
        };
        let entry = entries.pop().expect("one catalog record");
        info["backup"] = json!({"id":entry["id"],"file_size_human":entry["file_size_human"],"encrypted":entry["encrypted"],"created_at":entry["created_at"],"completed_at":entry["completed_at"],"local_available":entry["local_available"]});
        info["available_volumes"] = json!(volumes);
        info["volume_discovery"] = json!(discovery);
        info["remote_copies"] = entry["remote_sync_details"].clone();
        return Json(info).into_response();
    }
    match render(&state, rows, copies, history).await {
        Ok(mut rows) => Json(rows.pop().expect("one catalog record")).into_response(),
        Err(error) => *error,
    }
}

async fn page_rows(
    state: &HttpState,
    mut query: QueryBuilder<Sqlite>,
) -> Result<Vec<Value>, Box<Response>> {
    let mut db = state
        .auth
        .pool()
        .begin()
        .await
        .map_err(|_| Box::new(internal()))?;
    let rows = query
        .build()
        .fetch_all(&mut *db)
        .await
        .map_err(|_| Box::new(internal()))?;
    let copies = copies(&mut db, &rows)
        .await
        .map_err(|_| Box::new(internal()))?;
    db.commit().await.map_err(|_| Box::new(internal()))?;
    render(state, rows, copies, false).await
}
struct CopyInfo {
    id: i64,
    enabled: bool,
    dto: Value,
}
async fn copies(
    db: &mut SqliteConnection,
    rows: &[SqliteRow],
) -> Result<HashMap<i64, Vec<CopyInfo>>, sqlx::Error> {
    if rows.is_empty() {
        return Ok(HashMap::new());
    }
    let mut query: QueryBuilder<Sqlite> = QueryBuilder::new(
        "SELECT s.backup_id,s.storage_id,r.name,r.enabled,s.sync_status,s.synced_at,s.remote_path FROM backup_storage_syncs s JOIN remote_storages r ON r.id=s.storage_id WHERE s.backup_id IN (",
    );
    let mut ids = query.separated(",");
    for row in rows {
        ids.push_bind(row.try_get::<i64, _>("id")?);
    }
    query.push(") ORDER BY s.id");
    let mut result: HashMap<i64, Vec<CopyInfo>> = HashMap::new();
    for row in query.build().fetch_all(db).await? {
        let id = row.try_get::<i64, _>("storage_id")?;
        let status = row
            .try_get::<Option<String>, _>("sync_status")?
            .filter(|value| !value.is_empty())
            .unwrap_or_else(|| "unknown".into());
        result.entry(row.try_get("backup_id")?).or_default().push(CopyInfo { id, enabled: row.try_get::<Option<bool>,_>("enabled")?.unwrap_or(false), dto: json!({"storage_id":id,"storage_name":row.try_get::<String,_>("name")?,"status":status,"synced_at":timestamp(&row,"synced_at")?,"remote_path":row.try_get::<Option<String>,_>("remote_path")?}) });
    }
    Ok(result)
}
async fn render(
    state: &HttpState,
    rows: Vec<SqliteRow>,
    mut copies: HashMap<i64, Vec<CopyInfo>>,
    history: bool,
) -> Result<Vec<Value>, Box<Response>> {
    if rows.is_empty() {
        return Ok(vec![]);
    }
    let root = state.config.backup_base_path.clone();
    let work = state
        .probe_jobs
        .submit_blocking(move |_| {
            rows.into_iter()
                .map(|row| {
                    let id = row.try_get::<i64, _>("id")?;
                    record(
                        &row,
                        copies.remove(&id).unwrap_or_default(),
                        history,
                        FsPath::new(&root),
                    )
                })
                .collect::<Result<Vec<_>, sqlx::Error>>()
        })
        .map_err(|_| {
            Box::new(detail(
                StatusCode::SERVICE_UNAVAILABLE,
                "Catalog reader unavailable",
            ))
        })?;
    work.wait()
        .await
        .map_err(|_| Box::new(internal()))?
        .map_err(|_| Box::new(internal()))
}
fn format_size(size: i64) -> String {
    let mut size = size as f64;
    for unit in ["B", "KB", "MB", "GB", "TB"] {
        if size < 1024. {
            return format!("{size:.2} {unit}");
        }
        size /= 1024.;
    }
    format!("{size:.2} PB")
}
fn local(path: Option<&str>) -> (bool, &'static str) {
    let Some(path) = path.filter(|path| !path.is_empty()) else {
        return (false, "not_recorded");
    };
    let state = |path: &str| match std::fs::metadata(path) {
        Ok(meta) => (
            true,
            if meta.is_file() && nix::unistd::access(path, nix::unistd::AccessFlags::R_OK).is_ok() {
                "available"
            } else {
                "inaccessible"
            },
        ),
        Err(error) if error.kind() == ErrorKind::NotFound => (false, "missing"),
        Err(_) => (false, "inaccessible"),
    };
    let first = state(path);
    if first.0 {
        return first;
    }
    let sanitized = sanitized_path(path);
    if sanitized != path {
        let fallback = state(&sanitized);
        if fallback.0 {
            return fallback;
        }
    }
    first
}

fn sanitized_path(path: &str) -> String {
    let mut sanitized = String::new();
    for c in path.chars() {
        let c = if "<>:\"|?*".contains(c) { '_' } else { c };
        if c != '_' || !sanitized.ends_with('_') {
            sanitized.push(c);
        }
    }
    sanitized
}
fn stored_json<T: serde::de::DeserializeOwned>(
    row: &SqliteRow,
    column: &str,
) -> Result<Option<T>, sqlx::Error> {
    row.try_get::<Option<String>, _>(column)?
        .map(|raw| {
            serde_json::from_str::<Option<T>>(&raw)
                .map_err(|error| sqlx::Error::Decode(Box::new(error)))
        })
        .transpose()
        .map(Option::flatten)
}
fn record(
    row: &SqliteRow,
    mut copies: Vec<CopyInfo>,
    history: bool,
    root: &FsPath,
) -> Result<Value, sqlx::Error> {
    let target_name = row.try_get::<Option<String>, _>("target_name")?;
    let status = row.try_get::<String, _>("status")?.to_ascii_lowercase();
    let path: Option<String> = row.try_get("file_path")?;
    let size: Option<i64> = row.try_get("file_size")?;
    let error: Option<String> = row.try_get("error_message")?;
    let (available, mut local_state) = local(path.as_deref());
    let remote = copies.iter().any(|s| s.dto["status"] == "completed");
    let mut dto = json!({"id":row.try_get::<i64,_>("id")?,"target_id":row.try_get::<i64,_>("target_id")?,"target_name":target_name,"backup_type":row.try_get::<String,_>("backup_type")?.to_ascii_lowercase(),"status":status,"file_path":path,"file_size":size,"file_size_human":size.map(format_size),"checksum":row.try_get::<Option<String>,_>("checksum")?,"started_at":timestamp(row,"started_at")?,"completed_at":timestamp(row,"completed_at")?,"duration_seconds":row.try_get::<Option<i64>,_>("duration_seconds")?,"error_message":error,"encrypted":row.try_get::<Option<bool>,_>("encrypted")?.unwrap_or(false),"local_available":available,"remote_synced":remote,"created_at":timestamp(row,"created_at")?.unwrap_or_default()});
    if history {
        if local_state != "available"
            && stored_json::<Value>(row, "backup_metadata")?
                .is_some_and(|meta| meta["local_copy_removed"] == true)
        {
            local_state = "deleted";
        }
        let recorded = path.as_ref().is_some_and(|p| !p.is_empty())
            || size.is_some()
            || copies.iter().any(|s| {
                s.dto["status"] == "completed"
                    && s.dto["remote_path"].as_str().is_some_and(|p| !p.is_empty())
            });
        let archive_state = if recorded {
            "recorded"
        } else if matches!(status.as_str(), "failed" | "cancelled" | "pending") {
            "never_created"
        } else {
            "unknown"
        };
        let removed = row.try_get::<Option<String>, _>("removed_at")?.is_some();
        let retention = if removed {
            "Retained after plan removal; automatic plan retention disabled".to_owned()
        } else if let Some(id) = row
            .try_get::<Option<i64>, _>("retention_policy_id")?
            .filter(|&id| id != 0)
        {
            format!("Assigned retention policy #{id}")
        } else {
            "Default retention policy fallback".into()
        };
        let kind = row
            .try_get::<Option<String>, _>("target_type")?
            .unwrap_or_else(|| "unknown".into());
        let identity = match kind.as_str() {
            "container" => Some("container_name"),
            "stack" => Some("stack_name"),
            "volume" => Some("volume_name"),
            "path" => Some("host_path"),
            _ => None,
        };
        let identity = identity
            .map(|col| row.try_get::<Option<String>, _>(col))
            .transpose()?
            .flatten()
            .filter(|name| !name.is_empty())
            .or(target_name.clone())
            .unwrap_or_else(|| "Unknown source".into());
        let relative = path
            .as_deref()
            .and_then(|p| FsPath::new(p).strip_prefix(root).ok())
            .filter(|p| !p.components().any(|c| c == std::path::Component::ParentDir))
            .map(|p| {
                if p.as_os_str().is_empty() {
                    ".".into()
                } else {
                    p.to_string_lossy().into_owned()
                }
            });
        let local_available = local_state == "available";
        let remote_expected =
            stored_json::<Vec<i64>>(row, "remote_storage_ids")?.unwrap_or_default();
        let sync_error = error.as_deref().is_some_and(|error| {
            error.starts_with("Remote sync failed:")
                || error.starts_with("Remote storage sync failed:")
        });
        let can_sync = local_available
            && (status == "completed" || status == "failed" && sync_error)
            && target_name.is_some()
            && (copies.iter().any(|s| s.dto["status"] == "failed")
                || remote_expected
                    .iter()
                    .any(|id| !copies.iter().any(|s| s.id == *id)));
        let can_restore = status == "completed"
            && recorded
            && (local_available
                || copies
                    .iter()
                    .any(|s| s.enabled && s.dto["status"] == "completed"));
        dto.as_object_mut().unwrap().extend(json!({"source_type":kind,"source_identity":identity,"local_storage_path":relative,"plan_removed":removed || target_name.is_none(),"retention_context":retention,"archive_state":archive_state,"local_state":local_state,"stage":row.try_get::<Option<String>,_>("stage")?,"local_available":local_available,"can_restore":can_restore,"can_sync":can_sync}).as_object().unwrap().clone());
        for copy in &mut copies {
            copy.dto["availability"] = json!(if copy.enabled {
                "unknown"
            } else {
                "inaccessible"
            });
        }
    }
    dto["remote_sync_details"] = json!(copies.into_iter().map(|copy| copy.dto).collect::<Vec<_>>());
    Ok(dto)
}
fn restore_details(row: &SqliteRow) -> Result<Value, sqlx::Error> {
    let name: Option<String> = row.try_get("target_name")?;
    let kind: Option<String> = row.try_get("target_type")?;
    let container: Option<String> = row.try_get("container_name")?;
    let mut stop = vec![];
    if row
        .try_get::<Option<bool>, _>("stop_container")?
        .unwrap_or(false)
        && let Some(container) = container.as_ref().filter(|name| !name.is_empty())
    {
        stop.push(container.clone());
    }
    stop.extend(stored_json::<Vec<String>>(row, "dependencies")?.unwrap_or_default());
    Ok(
        json!({"target":{"id":name.as_ref().map(|_| row.get::<i64,_>("target_id")),"name":name.unwrap_or_else(|| "Unknown".into()),"target_type":kind,"volume_name":row.try_get::<Option<String>,_>("volume_name")?,"host_path":row.try_get::<Option<String>,_>("host_path")?,"container_name":container},"containers_to_stop":stop,"original_supported":matches!(kind.as_deref(),Some("path" | "volume"))}),
    )
}
