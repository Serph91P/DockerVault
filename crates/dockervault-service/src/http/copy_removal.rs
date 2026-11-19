use super::{
    HttpState,
    auth::{required_user, validation},
};
use crate::storage::{FileIdentity, local::open_download_source};
use axum::{
    Json,
    extract::State,
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use base64::{Engine, engine::general_purpose::URL_SAFE_NO_PAD};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use sqlx::{QueryBuilder, Sqlite};
use std::{
    collections::{BTreeSet, HashMap},
    io,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};

const TTL: Duration = Duration::from_secs(900);
const CAPACITY: usize = 100;

#[derive(Clone, Copy, Deserialize, Serialize, PartialEq, Eq)]
#[serde(rename_all = "lowercase")]
enum Scope {
    Local,
    Remote,
    All,
}

#[derive(Deserialize)]
pub(super) struct Selection {
    scope: Scope,
    backup_ids: Option<Vec<i64>>,
    target_id: Option<i64>,
    status: Option<String>,
}

#[derive(Clone, Serialize)]
struct Item {
    backup_id: i64,
    target_id: i64,
    target_name: String,
    copy: Scope,
    storage_id: Option<i64>,
    label: String,
    #[serde(skip)]
    local: Option<LocalCopy>,
}

type Backup = (
    i64,
    i64,
    String,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<String>,
    Option<i64>,
    Option<bool>,
);

#[derive(Clone)]
struct LocalCopy {
    archive: Option<String>,
    key: Option<String>,
    row_identity: (Option<String>, Option<String>, Option<i64>, Option<bool>),
    files: Option<Vec<(PathBuf, Option<FileIdentity>)>>,
}

fn inspect(
    root: &Path,
    archive: Option<&str>,
    key: Option<&str>,
) -> io::Result<Vec<(PathBuf, Option<FileIdentity>)>> {
    let mut files = Vec::new();
    if !root.is_absolute() {
        return Err(io::ErrorKind::InvalidInput.into());
    }
    for raw in [archive, key]
        .into_iter()
        .flatten()
        .filter(|path| !path.is_empty())
    {
        let path = std::path::absolute(raw)?;
        let relative = path
            .strip_prefix(root)
            .map_err(|_| io::ErrorKind::InvalidInput)?;
        let name = relative
            .to_str()
            .and_then(|p| crate::storage::relative_path(p, false))
            .ok_or(io::ErrorKind::InvalidInput)?;
        let relative = PathBuf::from(name);
        if files.iter().any(|(seen, _)| seen == &relative) {
            return Err(io::ErrorKind::InvalidInput.into());
        }
        let identity = match open_download_source(root, &relative) {
            Ok(file) => Some(FileIdentity::read(&file.metadata()?)?),
            Err(error) if error.kind() == io::ErrorKind::NotFound => None,
            Err(error) => return Err(error),
        };
        files.push((relative, identity));
    }
    Ok(files)
}

type SyncRow = (i64, i64, String);

struct Confirmation {
    expires: Instant,
    items: Vec<Item>,
    retained: Vec<Value>,
    result: tokio::sync::Mutex<Option<Value>>,
}

#[derive(Default)]
pub(super) struct Previews(HashMap<String, Arc<Confirmation>>);

impl Previews {
    fn insert(&mut self, items: Vec<Item>, retained: Vec<Value>) -> Result<String, StatusCode> {
        let now = Instant::now();
        self.0.retain(|_, item| item.expires > now);
        if self.0.len() >= CAPACITY {
            return Err(StatusCode::TOO_MANY_REQUESTS);
        }
        let mut bytes = [0u8; 32];
        getrandom::fill(&mut bytes).map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let token = URL_SAFE_NO_PAD.encode(bytes);
        let std::collections::hash_map::Entry::Vacant(slot) = self.0.entry(token.clone()) else {
            return Err(StatusCode::INTERNAL_SERVER_ERROR);
        };
        slot.insert(Arc::new(Confirmation {
            expires: now + TTL,
            items,
            retained,
            result: tokio::sync::Mutex::default(),
        }));
        Ok(token)
    }
    fn remember(
        &mut self,
        scope: Scope,
        archive_count: usize,
        items: Vec<Item>,
        retained: Vec<Value>,
    ) -> Result<Value, StatusCode> {
        let token = self.insert(items, retained)?;
        let saved = &self.0[&token];
        Ok(
            json!({"token":token,"scope":scope,"archive_count":archive_count,"copy_count":saved.items.len(),"items":saved.items,"retained":saved.retained,"expires_in_seconds":TTL.as_secs()}),
        )
    }
}

#[path = "copy_removal_execute.rs"]
mod execution;
pub(super) use execution::execute;
pub use execution::recover as recover_copy_removals;

fn detail(status: StatusCode, message: &'static str) -> Response {
    (status, Json(json!({"detail":message}))).into_response()
}

pub(super) async fn preview(
    State(state): State<HttpState>,
    headers: HeaderMap,
    input: Result<Json<Selection>, axum::extract::rejection::JsonRejection>,
) -> Response {
    if let Err(error) = required_user(&state, &headers).await {
        return *error;
    }
    let Ok(Json(selection)) = input else {
        return *validation(vec![
            json!({"field":"body","message":"Invalid removal selection"}),
        ]);
    };
    let ids = selection
        .backup_ids
        .as_ref()
        .map(|ids| ids.iter().copied().collect::<BTreeSet<_>>());
    if ids.as_ref().is_some_and(|ids| {
        ids.is_empty()
            || ids.first().is_some_and(|id| *id <= 0)
            || selection.target_id.is_some()
            || selection.status.is_some()
    }) || selection.target_id.is_some_and(|id| id <= 0)
        || selection.status.as_deref().is_some_and(|status| {
            !["pending", "running", "completed", "failed", "cancelled"].contains(&status)
        })
    {
        return *validation(vec![
            json!({"field":"body","message":"Invalid removal selection"}),
        ]);
    }
    let result = async {
        let mut db = state.auth.pool().begin().await?;
        let mut query = QueryBuilder::<Sqlite>::new("SELECT b.id,b.target_id,coalesce(t.name,'Historical archive') AS target_name,b.file_path,b.encryption_key_path,b.created_at,b.checksum,b.file_size,b.encrypted FROM backups b LEFT JOIN backup_targets t ON t.id=b.target_id WHERE 1=1");
        if let Some(id)=selection.target_id { query.push(" AND b.target_id=").push_bind(id); }
        if let Some(status)=selection.status { query.push(" AND b.status=").push_bind(status.to_ascii_uppercase()); }
        if let Some(ids)=&ids {
            query.push(" AND b.id IN (SELECT value FROM json_each(").push_bind(json!(ids).to_string()).push("))");
        }
        let backups = query.push(" ORDER BY b.id").build_query_as::<Backup>().fetch_all(&mut *db).await?;
        if ids.as_ref().is_some_and(|ids| ids.len()!=backups.len()) { return Ok::<_,sqlx::Error>(None); }
        let mut items = Vec::new();
        let mut retained = Vec::new();
        for (backup_id,target_id,target_name,file_path,key_path,created,checksum,size,encrypted) in &backups {
            let mut add = |copy,storage_id,label,local| {
                let item=Item{backup_id:*backup_id,target_id:*target_id,target_name:target_name.clone(),copy,storage_id,label,local};
                if selection.scope==Scope::All || selection.scope==copy { items.push(item); }
                else { let mut item=json!(item); item["outcome"]=json!("retained"); retained.push(item); }
            };
            if file_path.as_ref().is_some_and(|p| !p.is_empty()) || key_path.as_ref().is_some_and(|p| !p.is_empty()) {
                add(Scope::Local,None,"Local copy".into(),Some(LocalCopy{archive:file_path.clone(),key:key_path.clone(),row_identity:(created.clone(),checksum.clone(),*size,*encrypted),files:None}));
            }
            let rows = sqlx::query_as::<_,SyncRow>("SELECT s.id,s.storage_id,coalesce(r.name,'Storage '||s.storage_id) FROM backup_storage_syncs s LEFT JOIN remote_storages r ON r.id=s.storage_id WHERE s.backup_id=? ORDER BY s.id").bind(backup_id).fetch_all(&mut *db).await?;
            for (_,id,label) in rows { add(Scope::Remote,Some(id),label,None); }
        }
        db.commit().await?;
        Ok(Some((backups.len(),items,retained)))
    }.await;
    let (archive_count, items, retained) = match result {
        Ok(Some(result)) => result,
        Ok(None) => {
            return detail(
                StatusCode::CONFLICT,
                "Selection changed: archive not found; review again",
            );
        }
        Err(_) => {
            return detail(
                StatusCode::INTERNAL_SERVER_ERROR,
                "Could not read copy inventory",
            );
        }
    };
    let root = PathBuf::from(&state.config.backup_base_path);
    let Ok(work) = state.download_jobs.submit_blocking(move |_| {
        let mut items = items;
        for item in &mut items {
            if let Some(local) = &mut item.local {
                local.files = inspect(&root, local.archive.as_deref(), local.key.as_deref()).ok();
            }
        }
        items
    }) else {
        return detail(
            StatusCode::SERVICE_UNAVAILABLE,
            "Removal preview capacity exhausted",
        );
    };
    let Ok(items) = work.wait().await else {
        return detail(
            StatusCode::SERVICE_UNAVAILABLE,
            "Removal preview interrupted",
        );
    };
    match state.removal_previews.lock().unwrap().remember(
        selection.scope,
        archive_count,
        items,
        retained,
    ) {
        Ok(response) => Json(response).into_response(),
        Err(StatusCode::TOO_MANY_REQUESTS) => detail(
            StatusCode::TOO_MANY_REQUESTS,
            "Too many removal previews; wait for expiry",
        ),
        Err(_) => detail(
            StatusCode::INTERNAL_SERVER_ERROR,
            "Could not allocate removal preview",
        ),
    }
}
