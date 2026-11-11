//! Logical portable-state checks and public-custody normalization. Filesystem
//! observations and relational staging are still required by the service owner.
use crate::{
    cron::{CronError, Schedule, TimeZone},
    lossless_json::{JsonError, PythonJson},
    portable_records::PortableTables,
    rclone_package::{self, RcloneError},
    recovery_bundle,
    ssh_public::canonical_public_key,
    storage_validation::{required_fields, storage_errors},
    timestamp::Timestamp,
};
use indexmap::IndexMap;
use serde_json::{Value, json, value::RawValue};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum StateError {
    Invalid,
    Cancelled,
}
impl std::fmt::Display for StateError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Invalid or incompatible recovery state")
    }
}
impl std::error::Error for StateError {}
type Result<T> = std::result::Result<T, StateError>;
use StateError::Invalid;
// No Debug: plaintext portable credentials are present in these records.
pub struct ParsedState {
    pub tables: PortableTables,
    pub payload: PythonJson,
    pub backup_paths: Vec<Vec<String>>,
    pub preview: Value,
}
struct PosixPath {
    anchor: &'static str,
    parts: Vec<String>,
}
impl PosixPath {
    fn parse(text: &str, absolute: bool) -> Result<Self> {
        if text.is_empty()
            || text.contains(['\0', '\\'])
            || text.starts_with('/') != absolute
            || text.split('/').any(|p| p == "..")
        {
            return Err(Invalid);
        }
        let anchor = if text.starts_with("//") && !text.starts_with("///") {
            "//"
        } else if absolute {
            "/"
        } else {
            ""
        };
        Ok(Self {
            anchor,
            parts: text
                .split('/')
                .filter(|p| !p.is_empty() && *p != ".")
                .map(str::to_owned)
                .collect(),
        })
    }
    fn relative(&self, root: &Self) -> Result<Vec<String>> {
        if self.anchor != root.anchor
            || !self.parts.starts_with(&root.parts)
            || self.parts.len() == root.parts.len()
        {
            return Err(Invalid);
        }
        Ok(self.parts[root.parts.len()..].to_vec())
    }
    fn render(&self) -> String {
        let result = format!("{}{}", self.anchor, self.parts.join("/"));
        if result.is_empty() {
            ".".into()
        } else {
            result
        }
    }
}
fn digest(value: Value) -> Result<String> {
    let bytes = recovery_bundle::canonical(value).map_err(|_| Invalid)?;
    Ok(openssl::sha::sha256(&bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect())
}
fn json_error(error: JsonError) -> StateError {
    if error == JsonError::Cancelled {
        StateError::Cancelled
    } else {
        Invalid
    }
}
fn sha(bytes: &[u8]) -> String {
    openssl::sha::sha256(bytes)
        .iter()
        .map(|b| format!("{b:02x}"))
        .collect()
}
// Every subtree of our own canonical document is already canonical, including
// opaque strings. Hash those spans without round-tripping through Value.
fn manifest_canonical(canonical: &[u8]) -> Result<Value> {
    let root: IndexMap<&str, &RawValue> = serde_json::from_slice(canonical).map_err(|_| Invalid)?;
    let raw_tables: IndexMap<&str, &RawValue> =
        serde_json::from_str(root.get("tables").ok_or(Invalid)?.get()).map_err(|_| Invalid)?;
    let mut tables = serde_json::Map::new();
    for (name, rows) in raw_tables {
        let count = serde_json::from_str::<Vec<&RawValue>>(rows.get())
            .map_err(|_| Invalid)?
            .len();
        tables.insert(
            name.to_owned(),
            json!({"count":count,"sha256":sha(rows.get().as_bytes())}),
        );
    }
    Ok(
        json!({"tables":tables,"ssh_public_keys":sha(root.get("ssh_public_keys").ok_or(Invalid)?.get().as_bytes()),"rclone":sha(root.get("rclone").ok_or(Invalid)?.get().as_bytes())}),
    )
}
pub fn state_manifest(payload: &Value) -> Result<Value> {
    let raw = serde_json::to_vec(payload).map_err(|_| Invalid)?;
    let canonical = PythonJson::parse(&raw)
        .map_err(json_error)?
        .canonical(&|| false)
        .map_err(json_error)?;
    manifest_canonical(&canonical)
}
fn check_cancel(cancel: &dyn Fn() -> bool) -> Result<()> {
    if cancel() {
        Err(StateError::Cancelled)
    } else {
        Ok(())
    }
}
fn cron(text: &str, now: Timestamp, timezone: &TimeZone, cancel: &dyn Fn() -> bool) -> Result<()> {
    Schedule::parse(text)
        .and_then(|s| s.next(now, timezone, cancel))
        .map(|_| ())
        .map_err(|e| {
            if e == CronError::Cancelled {
                StateError::Cancelled
            } else {
                Invalid
            }
        })
}
pub fn parse_state(
    payload: Value,
    now: Timestamp,
    timezone: &TimeZone,
    options: &BTreeMap<String, BTreeSet<String>>,
    cancel: &dyn Fn() -> bool,
) -> Result<ParsedState> {
    check_cancel(cancel)?;
    let raw = serde_json::to_vec(&payload).map_err(|_| Invalid)?;
    parse_state_json(
        PythonJson::parse(&raw).map_err(json_error)?,
        now,
        timezone,
        options,
        cancel,
    )
}
pub fn parse_state_json(
    document: PythonJson,
    now: Timestamp,
    timezone: &TimeZone,
    options: &BTreeMap<String, BTreeSet<String>>,
    cancel: &dyn Fn() -> bool,
) -> Result<ParsedState> {
    check_cancel(cancel)?;
    let canonical = document.canonical(cancel).map_err(json_error)?;
    let mut expected_manifest = manifest_canonical(&canonical)?;
    let mut raw: IndexMap<String, Box<RawValue>> =
        serde_json::from_str(document.as_str()).map_err(|_| Invalid)?;
    let names = [
        "schema_version",
        "application",
        "compatibility_version",
        "exported_at",
        "backup_root",
        "tables",
        "ssh_public_keys",
        "rclone",
        "manifest",
    ];
    if raw.len() != names.len() || names.iter().any(|name| !raw.contains_key(*name)) {
        return Err(Invalid);
    }
    // Tables remain raw/typed; only the non-opaque header is decoded as Value.
    // No null/sentinel projection replaces user metadata.
    let mut payload = Value::Object(
        raw.iter()
            .filter(|(name, _)| name.as_str() != "tables")
            .map(|(name, value)| {
                serde_json::from_str(value.get())
                    .map(|value| (name.clone(), value))
                    .map_err(|_| Invalid)
            })
            .collect::<Result<serde_json::Map<String, Value>>>()?,
    );
    if payload["schema_version"].as_i64() != Some(1)
        || payload["compatibility_version"].as_i64() != Some(1)
        || payload["application"] != "dockervault"
    {
        return Err(Invalid);
    }
    Timestamp::parse(payload["exported_at"].as_str().ok_or(Invalid)?).map_err(|_| Invalid)?;
    let root = PosixPath::parse(payload["backup_root"].as_str().ok_or(Invalid)?, true)?;
    if !rclone_package::same(&expected_manifest, &payload["manifest"]) {
        return Err(Invalid);
    }
    let tables = PortableTables::parse_json(raw["tables"].get().as_bytes()).map_err(|_| Invalid)?;
    check_cancel(cancel)?;
    let storage_ids: BTreeSet<i64> = tables.remote_storages.iter().map(|s| s.id).collect();
    for row in &tables.app_settings {
        if ![
            "komodo_enabled",
            "komodo_api_url",
            "komodo_api_key",
            "komodo_api_secret",
            "storage_check_quota_before_upload",
            "encryption_onboarding_deferred",
        ]
        .contains(&row.key.as_str())
        {
            return Err(Invalid);
        }
    }
    for expression in tables
        .schedules
        .iter()
        .map(|s| s.cron_expression.as_str())
        .chain(
            tables
                .backup_schedules
                .iter()
                .map(|s| s.cron_expression.as_str()),
        )
    {
        cron(expression, now, timezone, cancel)?;
    }
    for row in &tables.backup_targets {
        check_cancel(cancel)?;
        if !["container", "volume", "path", "stack"].contains(&row.target_type.as_str()) {
            return Err(Invalid);
        }
        if !row.remote_storage_ids.as_array().is_some_and(|ids| {
            ids.iter()
                .all(|id| id.as_i64().is_some_and(|id| storage_ids.contains(&id)))
        }) {
            return Err(Invalid);
        }
        if let Some(text) = row.schedule_cron.as_deref().filter(|s| !s.is_empty()) {
            cron(text, now, timezone, cancel)?;
        }
        if let Some(text) = row.host_path.as_deref().filter(|s| !s.is_empty()) {
            PosixPath::parse(text, true)?;
        }
    }
    for row in &tables.remote_storages {
        check_cancel(cancel)?;
        if required_fields(&row.storage_type).is_none() {
            return Err(Invalid);
        }
        if let Some(text) = row.base_path.as_deref().filter(|s| !s.is_empty()) {
            PosixPath::parse(text, text.starts_with('/'))?;
        }
        if let Some(text) = row.ssh_key_path.as_deref().filter(|s| !s.is_empty()) {
            PosixPath::parse(text, true)?;
        }
    }
    let mut backup_paths = Vec::new();
    for row in &tables.backups {
        check_cancel(cancel)?;
        for text in [row.file_path.as_deref(), row.encryption_key_path.as_deref()]
            .into_iter()
            .flatten()
            .filter(|s| !s.is_empty())
        {
            backup_paths.push(PosixPath::parse(text, true)?.relative(&root)?);
        }
        if let (Some(archive), Some(key)) = (
            row.file_path.as_deref().filter(|s| !s.is_empty()),
            row.encryption_key_path.as_deref().filter(|s| !s.is_empty()),
        ) {
            let path = PosixPath::parse(archive, true)?;
            let leaf = path.parts.last().ok_or(Invalid)?;
            let expected = if leaf.len() > 4 && leaf.ends_with(".enc") {
                let rendered = path.render();
                format!("{}.key", rendered.strip_suffix(".enc").ok_or(Invalid)?)
            } else {
                format!("{archive}.key")
            };
            if key != expected {
                return Err(Invalid);
            }
        }
    }
    for row in &tables.backup_storage_syncs {
        if let Some(text) = row.remote_path.as_deref().filter(|s| !s.is_empty()) {
            PosixPath::parse(text, false)?;
        }
    }
    let empty = json!({"schema_version":1,"roots":[],"remotes":{},"files":{}});
    if tables
        .remote_storages
        .iter()
        .any(|s| s.storage_type == "rclone")
        || !rclone_package::same(&empty, &payload["rclone"])
    {
        rclone_package::validate(&payload["rclone"], &tables.remote_storages, options, cancel)
            .map_err(|e| {
                if e == RcloneError::Cancelled {
                    StateError::Cancelled
                } else {
                    Invalid
                }
            })?;
    }
    let mut used = BTreeSet::new();
    let mut public = Vec::new();
    for key in payload["ssh_public_keys"].as_array().ok_or(Invalid)? {
        check_cancel(cancel)?;
        let item = key.as_object().ok_or(Invalid)?;
        if item.len() != 2 || !item.contains_key("name") || !item.contains_key("public_key") {
            return Err(Invalid);
        }
        let name = key["name"].as_str().ok_or(Invalid)?;
        if name.is_empty()
            || name.len() > 40
            || !name
                .bytes()
                .all(|c| c.is_ascii_alphanumeric() || matches!(c, b'_' | b'-'))
            || !used.insert(name)
        {
            return Err(Invalid);
        }
        let canonical = canonical_public_key(key["public_key"].as_str().ok_or(Invalid)?.as_bytes())
            .map_err(|_| Invalid)?;
        public.push(json!({"name":name,"public_key":canonical}));
    }
    payload["ssh_public_keys"] = json!(public);
    // Only the SSH inventory changed; reuse our computed (not caller-provided)
    // table/rclone digests while rebinding the normalized public inventory.
    expected_manifest["ssh_public_keys"] = json!(digest(payload["ssh_public_keys"].clone())?);
    payload["manifest"] = expected_manifest;
    for name in ["ssh_public_keys", "manifest"] {
        raw.insert(
            name.to_owned(),
            RawValue::from_string(serde_json::to_string(&payload[name]).map_err(|_| Invalid)?)
                .map_err(|_| Invalid)?,
        );
    }
    let mut storages = Vec::new();
    for row in &tables.remote_storages {
        check_cancel(cancel)?;
        let values = serde_json::to_value(row).map_err(|_| Invalid)?;
        let ssh_private = ["ssh", "sftp"].contains(&row.storage_type.as_str())
            && row.ssh_key_path.as_ref().is_some_and(|s| !s.is_empty());
        let rclone_incomplete = if row.storage_type == "rclone" {
            payload["rclone"]["roots"]
                .as_array()
                .ok_or(Invalid)?
                .iter()
                .find(|r| {
                    r.get("storage_id")
                        .is_some_and(|id| rclone_package::integer_eq(id, row.id))
                })
                .is_none_or(|r| r["state"] != "portable")
        } else {
            false
        };
        storages.push(json!({"id":row.id,"type":row.storage_type,"state":if !storage_errors(&values).is_empty() || ssh_private || rclone_incomplete{"credentials_required"}else{"not_tested"}}));
    }
    let counts: serde_json::Map<String, Value> = payload["manifest"]["tables"]
        .as_object()
        .ok_or(Invalid)?
        .iter()
        .map(|(name, inventory)| (name.clone(), inventory["count"].clone()))
        .collect();
    let preview = json!({"counts":counts,"exported_at":payload["exported_at"],"storages":storages,"archives_included":false,"archive_private_keys_included":false});
    check_cancel(cancel)?;
    let normalized = serde_json::to_vec(&raw).map_err(|_| Invalid)?;
    Ok(ParsedState {
        tables,
        payload: PythonJson::parse(&normalized).map_err(json_error)?,
        backup_paths,
        preview,
    })
}
