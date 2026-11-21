use super::{HttpState, auth};
use crate::docker::DockerClient;
use axum::{
    Json,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::json;

struct Target {
    target_type: String,
    container_name: Option<String>,
    volume_name: Option<String>,
    host_path: Option<String>,
    stack_name: Option<String>,
    dependencies: Option<String>,
}

impl sqlx::FromRow<'_, sqlx::sqlite::SqliteRow> for Target {
    fn from_row(row: &sqlx::sqlite::SqliteRow) -> Result<Self, sqlx::Error> {
        use sqlx::Row;
        Ok(Self {
            target_type: row.try_get("target_type")?,
            container_name: row.try_get("container_name")?,
            volume_name: row.try_get("volume_name")?,
            host_path: row.try_get("host_path")?,
            stack_name: row.try_get("stack_name")?,
            dependencies: row.try_get("dependencies")?,
        })
    }
}

pub(super) async fn validate(State(state): State<HttpState>, Path(id): Path<String>) -> Response {
    let Some(id) = super::operations::query_integer(&id) else {
        return *auth::validation(vec![
            json!({"field":"path.backup_id","message":"Input should be a valid integer, unable to parse string as an integer"}),
        ]);
    };
    let db = state.auth.pool();
    let target = sqlx::query_as::<_, Target>("SELECT t.target_type,t.container_name,t.volume_name,t.host_path,t.stack_name,t.dependencies FROM backups b JOIN backup_targets t ON t.id=b.target_id WHERE b.id=?")
        .bind(id).fetch_optional(db).await;
    let target = match target {
        Ok(Some(target)) => target,
        Ok(None) => {
            return match sqlx::query_scalar::<_, bool>(
                "SELECT EXISTS(SELECT 1 FROM backups WHERE id=?)",
            )
            .bind(id)
            .fetch_one(db)
            .await
            {
                Ok(true) => error(StatusCode::NOT_FOUND, "Target not found"),
                Ok(false) => error(StatusCode::NOT_FOUND, "Backup not found"),
                Err(_) => internal(),
            };
        }
        Err(_) => return internal(),
    };
    let dependencies = match target
        .dependencies
        .as_deref()
        .map(serde_json::from_str::<Option<Vec<String>>>)
        .transpose()
    {
        Ok(deps) => deps.flatten().unwrap_or_default(),
        Err(_) => return internal(),
    };
    let mut issues = Vec::new();
    let docker_source = match target.target_type.as_str() {
        "container" => target.container_name.as_deref(),
        "volume" => target.volume_name.as_deref(),
        "stack" => target.stack_name.as_deref(),
        _ => None,
    }
    .filter(|name| !name.is_empty());
    let mut missing_dependencies = Vec::new();
    if docker_source.is_some() || !dependencies.is_empty() {
        let socket = state.config.docker_socket.clone();
        let source = docker_source.map(str::to_owned);
        let kind = target.target_type.clone();
        let Ok(work) = state.probe_jobs.submit_async(move |_| async move {
            let docker = DockerClient::connect(&socket).await?;
            let result = async {
                let containers =
                    if !dependencies.is_empty() || (source.is_some() && kind != "volume") {
                        docker.container_inventory(true).await?
                    } else {
                        Vec::new()
                    };
                let missing = match (kind.as_str(), source.as_deref()) {
                    ("container", Some(name)) if !containers.iter().any(|c| c["name"] == name) => {
                        Some(format!("Container '{name}' not found"))
                    }
                    ("stack", Some(name))
                        if !containers.iter().any(|c| c["compose_project"] == name) =>
                    {
                        Some(format!("Stack '{name}' not found"))
                    }
                    ("volume", Some(name))
                        if !docker.volume_names().await?.iter().any(|v| v == name) =>
                    {
                        Some(format!("Volume '{name}' not found"))
                    }
                    _ => None,
                };
                let missing_dependencies: Vec<_> = dependencies
                    .into_iter()
                    .filter(|name| !containers.iter().any(|c| c["name"] == *name))
                    .map(|name| format!("Dependency container '{name}' not found"))
                    .collect();
                Ok::<_, crate::docker::DockerError>((missing, missing_dependencies))
            }
            .await;
            docker.shutdown(None).await;
            result
        }) else {
            return internal();
        };
        let Ok(Ok((missing, deps))) = work.wait().await else {
            return internal();
        };
        issues.extend(missing);
        missing_dependencies = deps;
    }
    let root = state.config.backup_base_path.clone();
    let Ok(work) = state.auth.blocking().submit_blocking(move |_| {
        if target.target_type == "path"
            && let Some(path) = target.host_path.filter(|path| !path.is_empty())
        {
            if !std::path::Path::new(&path).exists() {
                issues.push(format!("Path '{path}' not found"));
            } else if nix::unistd::access(path.as_str(), nix::unistd::AccessFlags::R_OK).is_err() {
                issues.push(format!("Path '{path}' is not readable"));
            }
        }
        let space = std::fs::create_dir_all(&root)
            .and_then(|()| nix::sys::statvfs::statvfs(root.as_str()).map_err(std::io::Error::from));
        const GIB: u64 = 1024 * 1024 * 1024;
        match space {
            Ok(fs) => {
                let free = fs.blocks_available().saturating_mul(fs.fragment_size());
                if free < GIB {
                    issues.push(format!(
                        "Low disk space in backup directory: {:.2} GB free",
                        free as f64 / GIB as f64
                    ));
                }
            }
            Err(error) => issues.push(format!("Cannot check disk space: {error}")),
        }
        issues.extend(missing_dependencies);
        issues
    }) else {
        return internal();
    };
    match work.wait().await {
        Ok(issues) => Json(json!({"valid":issues.is_empty(),"issues":issues})).into_response(),
        Err(_) => internal(),
    }
}

fn error(status: StatusCode, detail: &str) -> Response {
    (status, Json(json!({"detail":detail}))).into_response()
}

fn internal() -> Response {
    error(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
}
