use super::HttpState;
use crate::docker::DockerClient;
use axum::{
    Json,
    extract::{Path, Query, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::collections::{BTreeMap, BTreeSet, VecDeque};

struct ComposeStack {
    name: String,
    items: Vec<Value>,
}

fn compose_stacks(items: Vec<Value>) -> Vec<ComposeStack> {
    let mut groups: Vec<ComposeStack> = Vec::new();
    let mut indices = BTreeMap::new();
    for item in items {
        let Some(name) = item["compose_project"]
            .as_str()
            .filter(|name| !name.is_empty())
        else {
            continue;
        };
        let position = *indices.entry(name.to_owned()).or_insert_with(|| {
            groups.push(ComposeStack {
                name: name.to_owned(),
                items: vec![],
            });
            groups.len() - 1
        });
        groups[position].items.push(item);
    }
    groups
}

fn values(value: &Value) -> impl Iterator<Item = &Value> {
    value.as_array().into_iter().flatten()
}

impl ComposeStack {
    fn dependencies(&self) -> Value {
        let names: Vec<_> = self
            .items
            .iter()
            .map(|item| item["name"].as_str().unwrap_or_default())
            .collect();
        let positions: BTreeMap<_, _> = names
            .iter()
            .copied()
            .enumerate()
            .map(|(i, name)| (name, i))
            .collect();
        let services: BTreeMap<_, _> = self
            .items
            .iter()
            .enumerate()
            .filter_map(|(i, item)| {
                item["compose_service"]
                    .as_str()
                    .filter(|s| !s.is_empty())
                    .map(|s| (s, i))
            })
            .collect();
        let mut incoming = vec![0; names.len()];
        let mut outgoing = vec![Vec::new(); names.len()];
        let mut dependencies = BTreeMap::new();
        for (i, item) in self.items.iter().enumerate() {
            let compose = item["labels"]["com.docker.compose.depends_on"]
                .as_str()
                .unwrap_or_default();
            let deps: BTreeSet<_> = compose
                .split(',')
                .map(|dep| dep.split(':').next().unwrap_or_default().trim())
                .chain(values(&item["depends_on"]).filter_map(Value::as_str))
                .filter(|dep| !dep.is_empty())
                .collect();
            for dep in &deps {
                if let Some(&index) = services.get(dep).or_else(|| positions.get(dep)) {
                    incoming[i] += 1;
                    outgoing[index].push(i);
                }
            }
            if !deps.is_empty() {
                dependencies.insert(names[i], deps);
            }
        }
        let mut queue: VecDeque<_> = (0..names.len()).filter(|&i| incoming[i] == 0).collect();
        let mut start = Vec::new();
        while let Some(i) = queue.pop_front() {
            start.push(names[i]);
            for &dependent in &outgoing[i] {
                incoming[dependent] -= 1;
                if incoming[dependent] == 0 {
                    queue.push_back(dependent);
                }
            }
        }
        start.extend(
            names
                .iter()
                .copied()
                .zip(incoming)
                .filter_map(|(name, count)| (count != 0).then_some(name)),
        );
        let stop: Vec<_> = start.iter().rev().collect();
        json!({"stack_name":self.name,"containers":names,"start_order":start,"stop_order":stop,"dependencies":dependencies})
    }

    fn list(&self) -> Value {
        let order = self.dependencies();
        let volumes: BTreeSet<_> = self
            .items
            .iter()
            .flat_map(|item| values(&item["mounts"]))
            .filter(|mount| mount["type"] == "volume")
            .filter_map(|mount| mount["name"].as_str())
            .filter(|name| !name.is_empty())
            .collect();
        let networks: BTreeSet<_> = self
            .items
            .iter()
            .flat_map(|item| values(&item["networks"]))
            .filter_map(Value::as_str)
            .collect();
        json!({"name":self.name,"containers":self.items,"volumes":volumes,"networks":networks,"start_order":order["start_order"],"stop_order":order["stop_order"]})
    }
}

pub(super) async fn stacks(State(state): State<HttpState>) -> Response {
    let Some(items) = read_containers(state, true).await else {
        return internal();
    };
    Json(
        compose_stacks(items)
            .iter()
            .map(ComposeStack::list)
            .collect::<Vec<_>>(),
    )
    .into_response()
}

pub(super) async fn stack_dependencies(
    State(state): State<HttpState>,
    Path(name): Path<String>,
) -> Response {
    let Some(items) = read_containers(state, true).await else {
        return internal();
    };
    match compose_stacks(items)
        .into_iter()
        .find(|stack| stack.name == name)
    {
        Some(stack) => Json(stack.dependencies()).into_response(),
        None => (
            StatusCode::NOT_FOUND,
            Json(json!({"detail":format!("Stack '{name}' not found")})),
        )
            .into_response(),
    }
}

pub(super) async fn containers(
    State(state): State<HttpState>,
    Query(query): Query<std::collections::HashMap<String, String>>,
) -> Response {
    let all = match query
        .get("all")
        .map(|value| value.to_ascii_lowercase())
        .as_deref()
    {
        None | Some("1" | "on" | "true" | "t" | "yes" | "y") => true,
        Some("0" | "off" | "false" | "f" | "no" | "n") => false,
        Some(_) => {
            return *super::auth::validation(vec![
                json!({"field":"query.all","message":"Input should be a valid boolean, unable to interpret input"}),
            ]);
        }
    };
    match read_containers(state, all).await {
        Some(items) => Json(items).into_response(),
        None => internal(),
    }
}

pub(super) async fn container(State(state): State<HttpState>, Path(id): Path<String>) -> Response {
    match read_containers(state, true).await {
        Some(items) => match items
            .into_iter()
            .find(|item| item["id"] == id || item["name"] == id)
        {
            Some(item) => Json(item).into_response(),
            None => (
                StatusCode::NOT_FOUND,
                Json(json!({"detail":"Container not found"})),
            )
                .into_response(),
        },
        None => internal(),
    }
}

async fn read_containers(state: HttpState, all: bool) -> Option<Vec<Value>> {
    let socket = state.config.docker_socket.clone();
    let result = state.probe_jobs.submit_async(move |_| async move {
        let docker = DockerClient::connect(&socket).await?;
        let result = docker.container_inventory(all).await;
        docker.shutdown(None).await;
        result
    });
    if let Ok(work) = result
        && let Ok(Ok(items)) = work.wait().await
    {
        return Some(items);
    }
    None
}

fn internal() -> Response {
    (
        StatusCode::INTERNAL_SERVER_ERROR,
        Json(json!({"detail":"Internal server error"})),
    )
        .into_response()
}

pub(super) async fn volumes(State(state): State<HttpState>) -> impl IntoResponse {
    let socket = state.config.docker_socket.clone();
    let result = state.probe_jobs.submit_async(move |_| async move {
        let docker = DockerClient::connect(&socket).await?;
        let result = docker.volume_inventory().await;
        docker.shutdown(None).await;
        result
    });
    if let Ok(work) = result
        && let Ok(Ok(volumes)) = work.wait().await
    {
        return Json(volumes).into_response();
    }
    internal()
}

pub(super) async fn health(State(state): State<HttpState>) -> impl IntoResponse {
    let socket = state.config.docker_socket.clone();
    let result = state.probe_jobs.submit_async(move |_| async move {
        let docker = DockerClient::connect(&socket).await?;
        let result = docker.ping().await;
        docker.shutdown(None).await;
        result
    });
    if let Ok(work) = result
        && matches!(work.wait().await, Ok(Ok(())))
    {
        return Json(json!({"status":"healthy"})).into_response();
    }
    (
        StatusCode::SERVICE_UNAVAILABLE,
        Json(json!({"detail":"Docker is not accessible"})),
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn stack_orders_dependencies_and_preserves_cycle_members() {
        let items = vec![
            json!({"name":"app-1","compose_project":"app","compose_service":"app","labels":{"com.docker.compose.depends_on":"db:service_healthy:true"},"depends_on":["outside"],"mounts":[{"type":"volume","name":"app-data"},{"type":"bind","name":"ignored"}],"networks":["app-net"]}),
            json!({"name":"db-1","compose_project":"app","compose_service":"db","depends_on":[]}),
            json!({"name":"cycle-1","compose_project":"app","depends_on":["cycle-1"]}),
            json!({"name":"outside","compose_project":null}),
        ];
        let mut stacks = compose_stacks(items);
        assert_eq!(stacks.len(), 1);
        let stack = stacks.pop().unwrap();
        assert_eq!(stack.list()["volumes"], json!(["app-data"]));
        assert_eq!(stack.list()["networks"], json!(["app-net"]));
        assert_eq!(
            stack.dependencies(),
            json!({"stack_name":"app","containers":["app-1","db-1","cycle-1"],"start_order":["db-1","app-1","cycle-1"],"stop_order":["cycle-1","app-1","db-1"],"dependencies":{"app-1":["db","outside"],"cycle-1":["cycle-1"]}})
        );
    }
}
