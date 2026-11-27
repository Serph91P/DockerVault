use super::{
    HttpState,
    auth::{required_user, validation},
    backups::format_size,
    operations::query_integer,
};
use axum::{
    Json,
    extract::{Path, State},
    http::HeaderMap,
    response::{IntoResponse, Response},
};
use serde_json::json;
use std::collections::{HashMap, VecDeque};
use time::{OffsetDateTime, format_description::well_known::Rfc3339};

// Like the Python engine, these are observations of this process, not catalog counters.
#[derive(Default)]
pub(super) struct Metrics {
    succeeded: u64,
    failed: u64,
    bytes: i64,
    last: Option<String>,
    samples: HashMap<i64, VecDeque<(f64, i64)>>,
}

impl Metrics {
    pub(super) fn record(&mut self, target: i64, seconds: f64, size: Option<i64>) {
        if let Some(size) = size {
            self.succeeded += 1;
            self.bytes = self.bytes.saturating_add(size);
        } else {
            self.failed += 1;
        }
        self.last = OffsetDateTime::now_utc()
            .format(&Rfc3339)
            .ok()
            .map(|stamp| stamp.replace('Z', "+00:00"));
        let samples = self.samples.entry(target).or_default();
        if samples.len() == 100 {
            samples.pop_front();
        }
        samples.push_back((seconds, size.unwrap_or_default()));
    }
}

pub(super) async fn summary(State(state): State<HttpState>, headers: HeaderMap) -> Response {
    if let Err(error) = required_user(&state, &headers).await {
        return *error;
    }
    let m = state.backup_metrics.lock().unwrap();
    let total = m.succeeded + m.failed;
    let rate = if total == 0 {
        0.0
    } else {
        (10000.0 * m.succeeded as f64 / total as f64).round_ties_even() / 100.0
    };
    Json(json!({"total_backups":total,"successful_backups":m.succeeded,"failed_backups":m.failed,"total_bytes_backed_up":m.bytes,"success_rate":rate,"last_backup_time":m.last})).into_response()
}

pub(super) async fn target(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Path(id): Path<String>,
) -> Response {
    if let Err(error) = required_user(&state, &headers).await {
        return *error;
    }
    let Some(id) = query_integer(&id) else {
        return *validation(vec![
            json!({"field":"path.target_id","message":"Input should be a valid integer, unable to parse string as an integer"}),
        ]);
    };
    let m = state.backup_metrics.lock().unwrap();
    let (duration, size, count) = m.samples.get(&id).map_or((None, None, 0), |samples| {
        let count = samples.len();
        let duration = samples.iter().map(|&(seconds, _)| seconds).sum::<f64>() / count as f64;
        let size = samples
            .iter()
            .map(|&(_, bytes)| i128::from(bytes))
            .sum::<i128>()
            / count as i128;
        (Some(duration), Some(size as i64), count)
    });
    Json(json!({"target_id":id,"average_duration_seconds":duration,"average_size_bytes":size,"average_size_human":size.filter(|&v| v != 0).map(format_size),"backup_count":count})).into_response()
}
