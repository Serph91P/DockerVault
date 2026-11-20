//! Offline canonical import validation and managed export assembly. Never reads host
//! files, discovers ambient credentials, executes commands or creates a filesystem.
use crate::{
    portable_records::RemoteStorage,
    rclone_path,
    rclone_ssh::{SshMaterial, canonical_material},
};
use base64::{Engine, engine::general_purpose::STANDARD};
use serde_json::{Map, Value, json};
use std::collections::{BTreeMap, BTreeSet};
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum RcloneError {
    Invalid,
    Cancelled,
}
use RcloneError::Invalid;
impl std::fmt::Display for RcloneError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("Invalid or unavailable portable rclone configuration")
    }
}
impl std::error::Error for RcloneError {}
type Result<T> = std::result::Result<T, RcloneError>;
fn keys(value: &Value, expected: &[&str]) -> bool {
    value
        .as_object()
        .is_some_and(|m| m.len() == expected.len() && expected.iter().all(|k| m.contains_key(*k)))
}
fn truth(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(b) => *b,
        Value::String(s) => !s.is_empty(),
        Value::Array(a) => !a.is_empty(),
        Value::Object(m) => !m.is_empty(),
        Value::Number(n) => n.as_f64() != Some(0.0),
    }
}
pub(crate) fn integer_eq(value: &Value, want: i64) -> bool {
    match value {
        Value::Bool(b) => i64::from(*b) == want,
        Value::Number(n) => {
            if let Some(n) = n.as_i64() {
                n == want
            } else if n.to_string().contains(['.', 'e', 'E']) {
                n.as_f64().is_some_and(|n| {
                    n.is_finite() && n.fract() == 0.0 && n as i128 == i128::from(want)
                })
            } else {
                false
            }
        }
        _ => false,
    }
}
// Python's metadata dictionary comparison permits numeric 1/true equality;
// published row IDs and the version field are independently strictly typed.
pub(crate) fn same(expected: &Value, actual: &Value) -> bool {
    if expected == actual {
        return true;
    }
    match expected {
        Value::Bool(b) => integer_eq(actual, i64::from(*b)),
        Value::Number(n) => n.as_i64().is_some_and(|n| integer_eq(actual, n)),
        Value::Array(a) => actual
            .as_array()
            .is_some_and(|b| a.len() == b.len() && a.iter().zip(b).all(|(a, b)| same(a, b))),
        Value::Object(a) => actual.as_object().is_some_and(|b| {
            a.len() == b.len() && a.iter().all(|(k, v)| b.get(k).is_some_and(|b| same(v, b)))
        }),
        _ => false,
    }
}
fn file_fields(kind: &str) -> &'static [&'static str] {
    match kind {
        "drive" | "google cloud storage" => &["service_account_file"],
        "s3" => &["shared_credentials_file"],
        "box" => &["box_config_file"],
        "azureblob" | "azurefiles" => &["client_certificate_path", "service_principal_file"],
        "oracleobjectstorage" => &["config_file", "sse_customer_key_file"],
        "sftp" => &["known_hosts_file", "pubkey_file"],
        _ => &[],
    }
}
fn command(kind: &str, field: &str) -> bool {
    field == "ssh"
        || (field.ends_with("_command")
            && !(kind == "sftp" && matches!(field, "md5sum_command" | "sha1sum_command")))
}
fn enabled(value: Option<&Value>) -> Result<bool> {
    let s = value.map_or(Ok("false"), |v| v.as_str().ok_or(Invalid))?;
    Ok(!s.is_empty() && s != "0" && !s.eq_ignore_ascii_case("false"))
}
fn canonical_reference(value: &str) -> Result<Option<String>> {
    let parsed = rclone_path::parse(value).map_err(|_| Invalid)?;
    if parsed.path.contains(['\0', '\\']) || parsed.path.split('/').any(|p| p == "..") {
        return Err(Invalid);
    }
    if let Some(name) = parsed.name {
        // Export expands inline/override nodes. Import must already be that exact
        // canonical packet, as the provider compares its recomputed whole packet.
        if name.starts_with(':') || !parsed.overrides.is_empty() {
            return Err(Invalid);
        }
        return Ok(Some(name));
    }
    if !parsed.path.starts_with('/') {
        return Err(Invalid);
    }
    let prefix = if parsed.path.starts_with("//") && !parsed.path.starts_with("///") {
        "//"
    } else {
        "/"
    };
    let canonical = format!(
        "{prefix}{}",
        parsed
            .path
            .split('/')
            .filter(|p| !p.is_empty() && *p != ".")
            .collect::<Vec<_>>()
            .join("/")
    );
    if canonical != parsed.path {
        return Err(Invalid);
    }
    Ok(None)
}
fn space_list(value: &str) -> Result<Vec<String>> {
    if value.is_empty() || value.contains(['\n', '\r']) {
        return Err(Invalid);
    }
    let bytes = value.as_bytes();
    let mut pos = 0;
    let mut result = Vec::new();
    loop {
        let mut field = Vec::new();
        if bytes.get(pos) == Some(&b'"') {
            pos += 1;
            let mut closed = false;
            while pos < bytes.len() {
                let b = bytes[pos];
                pos += 1;
                if b != b'"' {
                    field.push(b);
                } else if bytes.get(pos) == Some(&b'"') {
                    field.push(b'"');
                    pos += 1;
                } else {
                    closed = true;
                    break;
                }
            }
            if !closed || bytes.get(pos).is_some_and(|b| *b != b' ') {
                return Err(Invalid);
            }
        } else {
            while pos < bytes.len() && bytes[pos] != b' ' {
                if bytes[pos] == b'"' {
                    return Err(Invalid);
                }
                field.push(bytes[pos]);
                pos += 1;
            }
        }
        result.push(String::from_utf8(field).map_err(|_| Invalid)?);
        if pos == bytes.len() {
            break;
        }
        pos += 1;
    }
    Ok(result)
}
fn csv(values: &[String]) -> String {
    values
        .iter()
        .map(|v| {
            if v.contains([' ', '"', '\n', '\r']) || (v.is_empty() && values.len() == 1) {
                format!("\"{}\"", v.replace('"', "\"\""))
            } else {
                v.clone()
            }
        })
        .collect::<Vec<_>>()
        .join(" ")
}
struct Checker<'a> {
    remotes: &'a Map<String, Value>,
    files: &'a Map<String, Value>,
    options: &'a BTreeMap<String, BTreeSet<String>>,
    cancel: &'a dyn Fn() -> bool,
}
#[derive(Default)]
struct Collected {
    remotes: BTreeSet<String>,
    files: Map<String, Value>,
    external: bool,
}
impl Checker<'_> {
    fn kind(&self, name: &str) -> Result<&str> {
        self.remotes
            .get(name)
            .and_then(|r| r.get("type"))
            .and_then(Value::as_str)
            .ok_or(Invalid)
    }
    fn load(
        &self,
        key: &str,
        out: &mut Map<String, Value>,
        active: &mut BTreeSet<String>,
    ) -> Result<Vec<u8>> {
        if (self.cancel)() {
            return Err(RcloneError::Cancelled);
        }
        if active.contains(key) || active.len() > 32 || out.len() >= 256 {
            return Err(Invalid);
        }
        if let Some(record) = out.get(key) {
            return STANDARD
                .decode(record["content_b64"].as_str().ok_or(Invalid)?)
                .map_err(|_| Invalid);
        }
        let record = self.files.get(key).ok_or(Invalid)?;
        if !(keys(record, &["kind", "content_b64"])
            || keys(record, &["kind", "content_b64", "dependencies"]))
            || record["kind"] != "provider_credential"
        {
            return Err(Invalid);
        }
        let encoded = record["content_b64"].as_str().ok_or(Invalid)?;
        if encoded.len() > 1_398_104 {
            return Err(Invalid);
        }
        let data = STANDARD.decode(encoded).map_err(|_| Invalid)?;
        let digest = openssl::sha::sha256(&data)
            .iter()
            .map(|b| format!("{b:02x}"))
            .collect::<String>();
        if data.is_empty()
            || data.len() > 1024 * 1024
            || STANDARD.encode(&data) != encoded
            || digest != key
        {
            return Err(Invalid);
        }
        let empty = Vec::new();
        let dependencies = record
            .get("dependencies")
            .map_or(Ok(&empty), |v| v.as_array().ok_or(Invalid))?;
        if dependencies.len() > 32 {
            return Err(Invalid);
        }
        let mut seen = BTreeSet::new();
        active.insert(key.to_owned());
        for dependency in dependencies {
            let dependency = dependency.as_str().ok_or(Invalid)?;
            if !seen.insert(dependency) {
                return Err(Invalid);
            }
            let reference = format!("dv-recovery-file:{dependency}");
            if !data
                .windows(reference.len())
                .any(|w| w == reference.as_bytes())
            {
                return Err(Invalid);
            }
            self.load(dependency, out, active)?;
        }
        active.remove(key);
        out.insert(key.to_owned(), record.clone());
        Ok(data)
    }
    fn visit(
        &self,
        name: &str,
        depth: usize,
        active: &mut BTreeSet<String>,
        out: &mut Collected,
    ) -> Result<()> {
        if (self.cancel)() {
            return Err(RcloneError::Cancelled);
        }
        if depth > 32 || active.contains(name) || !rclone_path::valid_name(name) {
            return Err(Invalid);
        }
        if out.remotes.contains(name) {
            return Ok(());
        }
        let node = self
            .remotes
            .get(name)
            .and_then(Value::as_object)
            .ok_or(Invalid)?;
        if node.len() > 256 {
            return Err(Invalid);
        }
        let kind = self.kind(name)?;
        let options = self.options.get(kind).ok_or(Invalid)?;
        for (field, value) in node {
            if field != "type" && !options.contains(field) {
                return Err(Invalid);
            }
            if let Some(s) = value.as_str() {
                if s.chars().count() > 1024 * 1024 {
                    return Err(Invalid);
                }
            } else if !file_fields(kind).contains(&field.as_str()) {
                return Err(Invalid);
            }
            if truth(value) && command(kind, field) {
                return Err(Invalid);
            }
        }
        if kind == "sftp" {
            for field in ["key_pem", "key_file", "key_file_pass", "key_use_agent"] {
                if node.get(field).is_some_and(|v| {
                    !v.is_null() && !matches!(v.as_str(), Some("" | "false" | "0"))
                }) {
                    return Err(Invalid);
                }
            }
            if let Some(s) = node.get("pubkey").filter(|v| truth(v)) {
                let s = s.as_str().ok_or(Invalid)?;
                if canonical_material(s.as_bytes(), SshMaterial::PublicIdentity)
                    .map_err(|_| Invalid)?
                    != s.as_bytes()
                {
                    return Err(Invalid);
                }
            }
        }
        if kind == "oracleobjectstorage" {
            let provider = node
                .get("provider")
                .map_or("env_auth", |v| v.as_str().unwrap_or(""));
            if matches!(
                provider,
                "env_auth"
                    | "instance_principal_auth"
                    | "workload_identity_auth"
                    | "resource_principal_auth"
            ) || (provider == "user_principal_auth"
                && !node.get("config_file").is_some_and(truth))
            {
                out.external = true;
            }
        }
        if enabled(node.get("env_auth"))?
            && !file_fields(kind)
                .iter()
                .any(|f| node.get(*f).is_some_and(truth))
            && !(matches!(kind, "drive" | "google cloud storage")
                && node.get("service_account_credentials").is_some_and(truth))
        {
            out.external = true;
        }
        active.insert(name.to_owned());
        if matches!(
            kind,
            "alias" | "crypt" | "chunker" | "compress" | "hasher" | "archive" | "cache"
        ) {
            let reference = node.get("remote").and_then(Value::as_str).unwrap_or("");
            if let Some(dependency) = canonical_reference(reference)? {
                self.visit(&dependency, depth + 1, active, out)?;
            }
        } else if matches!(kind, "union" | "combine") {
            let original = node.get("upstreams").and_then(Value::as_str).unwrap_or("");
            let mut normalized = Vec::new();
            for upstream in space_list(original)? {
                let (mount, upstream) = if kind == "combine" {
                    let (mount, path) = upstream.split_once('=').ok_or(Invalid)?;
                    if mount.is_empty() || mount.contains('/') || matches!(mount, "." | "..") {
                        return Err(Invalid);
                    }
                    (format!("{mount}="), path)
                } else {
                    (String::new(), upstream.as_str())
                };
                let (reference, suffix) = if kind == "union" {
                    [":writeback", ":ro", ":nc"]
                        .into_iter()
                        .find_map(|flag| upstream.strip_suffix(flag).map(|v| (v, flag)))
                        .unwrap_or((upstream, ""))
                } else {
                    (upstream, "")
                };
                if let Some(dependency) = canonical_reference(reference)? {
                    self.visit(&dependency, depth + 1, active, out)?;
                }
                normalized.push(format!("{mount}{reference}{suffix}"));
            }
            if csv(&normalized) != original {
                return Err(Invalid);
            }
        }
        for field in file_fields(kind) {
            if let Some(value) = node.get(*field).filter(|v| truth(v)) {
                if !keys(value, &["file_ref"]) {
                    return Err(Invalid);
                }
                let key = value["file_ref"].as_str().ok_or(Invalid)?;
                let data = self.load(key, &mut out.files, &mut BTreeSet::new())?;
                if kind == "sftp" {
                    if out.files[key].get("dependencies").is_some_and(truth) {
                        return Err(Invalid);
                    }
                    let mode = if *field == "pubkey_file" {
                        SshMaterial::PublicIdentity
                    } else {
                        SshMaterial::KnownHosts
                    };
                    if canonical_material(&data, mode).map_err(|_| Invalid)? != data {
                        return Err(Invalid);
                    }
                }
            }
        }
        active.remove(name);
        out.remotes.insert(name.to_owned());
        Ok(())
    }
}
/// Validate and collect only referenced credential records using the shared
/// digest/Base64/dependency checker, without reading any host credential paths.
pub fn collect_materials(
    files: &Value,
    references: &[String],
    cancel: &dyn Fn() -> bool,
) -> Result<Value> {
    if cancel() {
        return Err(RcloneError::Cancelled);
    }
    let remotes = Map::new();
    let options = BTreeMap::new();
    let checker = Checker {
        remotes: &remotes,
        files: files.as_object().ok_or(Invalid)?,
        options: &options,
        cancel,
    };
    let mut out = Map::new();
    for reference in references {
        if cancel() {
            return Err(RcloneError::Cancelled);
        }
        if !out.contains_key(reference) {
            checker.load(reference, &mut out, &mut BTreeSet::new())?;
        }
    }
    Ok(Value::Object(out))
}

/// Assemble an export from canonical managed remotes, pruning unreferenced nodes.
/// Does not normalize raw external config, discover credentials or read host files.
/// Reuses the import checker's graph, material and explicit-requirement rules.
pub fn assemble_canonical(
    remotes: &Value,
    files: &Value,
    storages: &[RemoteStorage],
    roots: &Value,
    options: &BTreeMap<String, BTreeSet<String>>,
    cancel: &dyn Fn() -> bool,
) -> Result<Value> {
    if cancel() {
        return Err(RcloneError::Cancelled);
    }
    let roots = roots.as_array().ok_or(Invalid)?;
    let remotes = remotes.as_object().ok_or(Invalid)?;
    let files = files.as_object().ok_or(Invalid)?;
    let storages = storages
        .iter()
        .filter(|s| s.storage_type == "rclone")
        .collect::<Vec<_>>();
    if remotes.len() > 256 {
        return Err(Invalid);
    }
    let checker = Checker {
        remotes,
        files,
        options,
        cancel,
    };
    let mut all_remotes = BTreeSet::new();
    let mut all_files = Map::new();
    let mut output_roots = Vec::new();
    let empty_source = Value::Null;
    for storage in storages {
        if cancel() {
            return Err(RcloneError::Cancelled);
        }
        let source = roots
            .iter()
            .rev()
            .find(|r| {
                r.get("storage_id")
                    .is_some_and(|v| integer_eq(v, storage.id))
            })
            .unwrap_or(&empty_source);
        let empty = Vec::new();
        let mut requirements = source
            .get("ssh_requirements")
            .map_or(Ok(empty.clone()), |v| v.as_array().cloned().ok_or(Invalid))?;
        let mut commands = source
            .get("external_commands")
            .map_or(Ok(empty), |v| v.as_array().cloned().ok_or(Invalid))?;
        for requirement in &requirements {
            if !keys(requirement, &["remote", "key_path", "agent"])
                || !requirement["agent"].is_boolean()
                || checker.kind(requirement["remote"].as_str().ok_or(Invalid)?)? != "sftp"
            {
                return Err(Invalid);
            }
            if !requirement["key_path"].is_null() {
                let path = requirement["key_path"].as_str().ok_or(Invalid)?;
                if !path.starts_with('/')
                    || path.contains('\0')
                    || path.split('/').any(|p| p == "..")
                {
                    return Err(Invalid);
                }
            }
        }
        for value in &commands {
            if !keys(value, &["remote", "field"]) {
                return Err(Invalid);
            }
            let kind = checker.kind(value["remote"].as_str().ok_or(Invalid)?)?;
            let field = value["field"].as_str().ok_or(Invalid)?;
            if !options.get(kind).is_some_and(|o| o.contains(field)) || !command(kind, field) {
                return Err(Invalid);
            }
        }
        let mut expected =
            json!({"storage_id":storage.id,"remote_name":storage.rclone_remote,"state":"portable"});
        let mut collected = Collected::default();
        let outcome = storage
            .rclone_remote
            .as_deref()
            .ok_or(Invalid)
            .and_then(|name| checker.visit(name, 0, &mut BTreeSet::new(), &mut collected));
        if outcome == Err(RcloneError::Cancelled) {
            return Err(RcloneError::Cancelled);
        }
        if outcome.is_err() {
            expected["state"] = json!("credentials_required");
            expected["reason"] = json!("external_or_unsafe_dependency");
        } else {
            if !requirements.is_empty() {
                requirements.sort_by(|a, b| a["remote"].as_str().cmp(&b["remote"].as_str()));
                expected["state"] = json!("credentials_required");
                expected["reason"] = json!("ssh_private_key_required");
                expected["ssh_requirements"] = Value::Array(requirements);
            }
            if collected.external || !commands.is_empty() {
                expected["state"] = json!("credentials_required");
                expected["reason"] = json!("external_credentials_required");
                if collected.external {
                    expected["external_identity_required"] = json!(true);
                }
                if !commands.is_empty() {
                    commands.sort_by(|a, b| {
                        (a["remote"].as_str(), a["field"].as_str())
                            .cmp(&(b["remote"].as_str(), b["field"].as_str()))
                    });
                    expected["external_commands"] = Value::Array(commands);
                }
            }
            all_remotes.extend(collected.remotes);
            all_files.extend(collected.files);
        }
        output_roots.push(expected);
    }
    if cancel() {
        return Err(RcloneError::Cancelled);
    }
    let selected: Map<String, Value> = all_remotes
        .into_iter()
        .map(|name| {
            let value = remotes[&name].clone();
            (name, value)
        })
        .collect();
    Ok(json!({"schema_version":1,"roots":output_roots,"remotes":selected,"files":all_files}))
}

/// Combine already normalized snapshots without conflating same-named remotes.
pub fn merge_snapshots(
    managed: Value,
    external: Value,
    storages: &mut [RemoteStorage],
    options: &BTreeMap<String, BTreeSet<String>>,
    cancel: &dyn Fn() -> bool,
) -> Result<Value> {
    let names = |packet: &Value| -> Result<BTreeSet<String>> {
        let mut names: BTreeSet<_> = packet["remotes"]
            .as_object()
            .ok_or(Invalid)?
            .keys()
            .cloned()
            .collect();
        for root in packet["roots"].as_array().ok_or(Invalid)? {
            names.insert(root["remote_name"].as_str().ok_or(Invalid)?.to_owned());
        }
        Ok(names)
    };
    let occupied = names(&managed)?;
    let external_names = names(&external)?;
    let mut reserved: BTreeSet<_> = occupied.union(&external_names).cloned().collect();
    let mut renamed = BTreeMap::new();
    for name in occupied.intersection(&external_names) {
        let hash = openssl::sha::sha256(name.as_bytes())
            .iter()
            .map(|byte| format!("{byte:02x}"))
            .collect::<String>();
        let mut replacement = format!("__dv_external_{hash}");
        while reserved.contains(&replacement) {
            replacement.push('_');
        }
        reserved.insert(replacement.clone());
        renamed.insert(name.clone(), replacement);
    }
    let rename = |name: &str| {
        renamed
            .get(name)
            .cloned()
            .unwrap_or_else(|| name.to_owned())
    };
    let reference = |value: &str| -> Result<String> {
        Ok(match canonical_reference(value)? {
            Some(name) => format!(
                "{}:{}",
                rename(&name),
                rclone_path::parse(value).map_err(|_| Invalid)?.path
            ),
            None => value.to_owned(),
        })
    };
    let mut remotes = managed["remotes"].as_object().ok_or(Invalid)?.clone();
    for (name, node) in external["remotes"].as_object().ok_or(Invalid)? {
        if cancel() {
            return Err(RcloneError::Cancelled);
        }
        let mut node = node.clone();
        match node["type"].as_str().ok_or(Invalid)? {
            "alias" | "crypt" | "chunker" | "compress" | "hasher" | "archive" | "cache" => {
                node["remote"] = json!(reference(node["remote"].as_str().ok_or(Invalid)?)?);
            }
            kind @ ("combine" | "union") => {
                let mut output = Vec::new();
                for value in space_list(node["upstreams"].as_str().ok_or(Invalid)?)? {
                    if kind == "combine" {
                        let (mount, target) = value.split_once('=').ok_or(Invalid)?;
                        output.push(format!("{mount}={}", reference(target)?));
                    } else {
                        let (target, suffix) = [":writeback", ":ro", ":nc"]
                            .into_iter()
                            .find_map(|flag| value.strip_suffix(flag).map(|v| (v, flag)))
                            .unwrap_or((&value, ""));
                        output.push(format!("{}{suffix}", reference(target)?));
                    }
                }
                node["upstreams"] = json!(csv(&output));
            }
            _ => {}
        }
        if remotes.insert(rename(name), node).is_some() {
            return Err(Invalid);
        }
    }
    let mut files = managed["files"].as_object().ok_or(Invalid)?.clone();
    for (id, value) in external["files"].as_object().ok_or(Invalid)? {
        if files.get(id).is_some_and(|old| old != value) {
            return Err(Invalid);
        }
        files.insert(id.clone(), value.clone());
    }
    let mut roots = BTreeMap::new();
    for (packet, is_external) in [(managed, false), (external, true)] {
        for root in packet["roots"].as_array().ok_or(Invalid)? {
            let mut root = root.clone();
            if is_external {
                root["remote_name"] = json!(rename(root["remote_name"].as_str().ok_or(Invalid)?));
                for field in ["ssh_requirements", "external_commands"] {
                    if let Some(values) = root.get_mut(field) {
                        let values = values.as_array_mut().ok_or(Invalid)?;
                        for value in &mut *values {
                            value["remote"] =
                                json!(rename(value["remote"].as_str().ok_or(Invalid)?));
                        }
                        values.sort_by(|a, b| {
                            (a["remote"].as_str(), a["field"].as_str())
                                .cmp(&(b["remote"].as_str(), b["field"].as_str()))
                        });
                    }
                }
            }
            if roots
                .insert(root["storage_id"].as_i64().ok_or(Invalid)?, root)
                .is_some()
            {
                return Err(Invalid);
            }
        }
    }
    let mut ordered = Vec::new();
    for row in storages
        .iter_mut()
        .filter(|row| row.storage_type == "rclone")
    {
        let root = roots.remove(&row.id).ok_or(Invalid)?;
        row.rclone_remote = Some(root["remote_name"].as_str().ok_or(Invalid)?.to_owned());
        ordered.push(root);
    }
    if !roots.is_empty() {
        return Err(Invalid);
    }
    let packet = json!({"schema_version":1,"roots":ordered,"remotes":remotes,"files":files});
    validate(&packet, storages, options, cancel)?;
    Ok(packet)
}

pub fn validate(
    payload: &Value,
    storages: &[RemoteStorage],
    options: &BTreeMap<String, BTreeSet<String>>,
    cancel: &dyn Fn() -> bool,
) -> Result<()> {
    if cancel() {
        return Err(RcloneError::Cancelled);
    }
    if !keys(payload, &["schema_version", "roots", "remotes", "files"])
        || payload["schema_version"].as_i64() != Some(1)
        || payload["roots"].as_array().is_none_or(|roots| {
            roots.len()
                != storages
                    .iter()
                    .filter(|s| s.storage_type == "rclone")
                    .count()
        })
        || payload["files"]
            .as_object()
            .is_none_or(|files| files.len() > 256)
    {
        return Err(Invalid);
    }
    let expected = assemble_canonical(
        &payload["remotes"],
        &payload["files"],
        storages,
        &payload["roots"],
        options,
        cancel,
    )?;
    if same(&expected, payload) {
        Ok(())
    } else {
        Err(Invalid)
    }
}
