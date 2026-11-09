//! Versioned input for the fixed privileged archive executable, not an HTTP DTO.
mod create;
pub use create::{Report, create_archive};
use dockervault_core::filters::Filters;
use serde::Deserialize;
use serde_json::Value;
use std::collections::BTreeMap;
use std::path::{Component, Path, PathBuf};

pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Request {
    version: u32,
    pub dest: PathBuf,
    pub sources: Vec<(String, PathBuf)>,
    #[serde(default = "yes")]
    pub compress: bool,
    #[serde(default)]
    include_paths: Option<Vec<String>>,
    #[serde(default)]
    exclude_paths: Option<Vec<String>>,
    #[serde(default)]
    per_volume_rules: BTreeMap<String, Value>,
    #[serde(default)]
    pub allowed_sources: Vec<PathBuf>,
}

impl Request {
    /// Parsing checks syntax only; opened-FD containment must precede filesystem I/O.
    pub fn parse(input: &[u8]) -> Result<Self, &'static str> {
        if input.len() > MAX_REQUEST_BYTES {
            return Err("Archive worker request is too large");
        }
        let mut value: Self =
            serde_json::from_slice(input).map_err(|_| "Invalid archive worker request")?;
        if value.version != 1 {
            return Err("Unsupported archive worker protocol version");
        }
        if !safe_absolute(&value.dest)
            || !value.dest.starts_with("/backups")
            || value.dest == Path::new("/backups")
        {
            return Err("Destination is outside the backup directory");
        }
        for (name, source) in &mut value.sources {
            *name = name.trim_start_matches('/').to_owned();
            if name.contains('\0')
                || Path::new(name).components().any(|c| {
                    matches!(
                        c,
                        Component::ParentDir | Component::RootDir | Component::Prefix(_)
                    )
                })
                || !safe_absolute(source)
            {
                return Err("Invalid archive source path or root");
            }
        }
        if value.allowed_sources.iter().any(|p| !safe_absolute(p)) {
            return Err("Invalid allowed source prefix");
        }
        // Validate every selected rule before a caller can begin an archive.
        for (name, _) in &value.sources {
            value.filters_for(name)?;
        }
        Ok(value)
    }

    pub fn filters_for(&self, archive_name: &str) -> Result<Filters, &'static str> {
        let key = archive_name.trim_start_matches('/');
        let basename = key.rsplit('/').next().unwrap_or("");
        let selected = self
            .per_volume_rules
            .get(key)
            .filter(|v| truthy(v))
            .or_else(|| self.per_volume_rules.get(basename).filter(|v| truthy(v)));
        if let Some(selected) = selected {
            let object = selected.as_object().ok_or("Invalid selected volume rule")?;
            let paths = |name: &str| -> Result<Vec<String>, &'static str> {
                match object.get(name) {
                    None | Some(Value::Null) => Ok(Vec::new()),
                    Some(Value::Array(values)) => values
                        .iter()
                        .map(|v| v.as_str().map(str::to_owned).ok_or("Invalid volume filter"))
                        .collect(),
                    _ => Err("Invalid volume filter"),
                }
            };
            Ok(Filters::new(
                &paths("include_paths")?,
                &paths("exclude_paths")?,
            ))
        } else {
            Ok(Filters::new(
                self.include_paths.as_deref().unwrap_or_default(),
                self.exclude_paths.as_deref().unwrap_or_default(),
            ))
        }
    }
}

fn safe_absolute(path: &Path) -> bool {
    path.is_absolute()
        && !path.as_os_str().as_encoded_bytes().contains(&0)
        && !path.components().any(|c| matches!(c, Component::ParentDir))
}

fn truthy(value: &Value) -> bool {
    match value {
        Value::Null => false,
        Value::Bool(value) => *value,
        Value::Number(value) => value.as_f64().is_none_or(|n| n != 0.0),
        Value::String(value) => !value.is_empty(),
        Value::Array(value) => !value.is_empty(),
        Value::Object(value) => !value.is_empty(),
    }
}

fn yes() -> bool {
    true
}
