use super::{HttpState, auth, operations::query_integer};
use axum::{
    Json,
    extract::{Query, State},
    http::{HeaderMap, StatusCode},
    response::{IntoResponse, Response},
};
use serde_json::{Value, json};
use std::collections::HashMap;
use time::{Duration, UtcDateTime, format_description::well_known::Rfc3339};

pub(super) async fn summary(
    State(state): State<HttpState>,
    headers: HeaderMap,
    Query(query): Query<HashMap<String, String>>,
) -> Response {
    let user = match auth::required_user(&state, &headers).await {
        Ok(user) => user,
        Err(error) => return *error,
    };
    let limit = match query.get("limit") {
        None => 5,
        Some(raw) => match query_integer(raw) {
            Some(n @ 1..=20) => n,
            value => {
                return *auth::validation(vec![
                    json!({"field":"query.limit","message":match value {
                        Some(n) if n < 1 => "Input should be greater than or equal to 1",
                        Some(_) => "Input should be less than or equal to 20",
                        None => "Input should be a valid integer, unable to parse string as an integer",
                    }}),
                ]);
            }
        },
    };
    match snapshot(&state, user.id, limit).await {
        Ok(value) => Json(value).into_response(),
        Err(_) => (
            StatusCode::INTERNAL_SERVER_ERROR,
            Json(json!({"detail":"Internal server error"})),
        )
            .into_response(),
    }
}

async fn snapshot(state: &HttpState, user: i64, limit: i64) -> Result<Value, sqlx::Error> {
    let now = UtcDateTime::now();
    let utc = |date: UtcDateTime| date.format(&Rfc3339).expect("current UTC date");
    let live = state.live_backups.lock().unwrap().clone();
    let mut tx = state.auth.pool().begin().await?;
    let (total, enabled, manual, automatic_enabled): (i64,i64,i64,i64) = sqlx::query_as("SELECT count(*),count(*) FILTER(WHERE t.enabled=1),count(*) FILTER(WHERE t.schedule_id IS NULL AND coalesce(t.schedule_cron,'')=''),count(*) FILTER(WHERE t.enabled=1 AND (s.enabled=1 OR (t.schedule_id IS NULL AND coalesce(t.schedule_cron,'')!=''))) FROM backup_targets t LEFT JOIN schedules s ON s.id=t.schedule_id WHERE t.removed_at IS NULL").fetch_one(&mut *tx).await?;
    let mut gaps = Vec::new();
    for completed in [false, true] {
        let sql = "WITH gaps AS (SELECT t.id,t.name,t.enabled FROM backup_targets t WHERE t.removed_at IS NULL AND NOT EXISTS(SELECT 1 FROM backups b WHERE b.target_id=t.id AND (NOT ?1 OR b.status='COMPLETED'))) SELECT count(*),count(*) FILTER(WHERE enabled=1),(SELECT json_group_array(json_object('plan_id',id,'name',name)) FROM (SELECT id,name FROM gaps ORDER BY id LIMIT ?2)) FROM gaps";
        let (total, enabled, items): (i64, i64, String) = sqlx::query_as(sql)
            .bind(completed)
            .bind(limit)
            .fetch_one(&mut *tx)
            .await?;
        let items: Value =
            serde_json::from_str(&items).map_err(|e| sqlx::Error::Decode(Box::new(e)))?;
        gaps.push(json!({"total":total,"enabled":enabled,"items":items}));
    }
    let ids = json!(live.keys().collect::<Vec<_>>()).to_string();
    let (pending,running,active):(i64,i64,i64) = sqlx::query_as("SELECT count(*) FILTER(WHERE status='PENDING'),count(*) FILTER(WHERE status='RUNNING'),count(*) FILTER(WHERE status IN ('PENDING','RUNNING') OR id IN (SELECT value FROM json_each(?))) FROM backups").bind(&ids).fetch_one(&mut *tx).await?;
    type ActivityRow = (i64, Option<i64>, Option<String>, Option<String>);
    let rows: Vec<ActivityRow> = sqlx::query_as("SELECT b.id,b.target_id,t.name,lower(b.status) FROM backups b LEFT JOIN backup_targets t ON t.id=b.target_id WHERE b.status IN ('PENDING','RUNNING') OR b.id IN (SELECT value FROM json_each(?)) ORDER BY b.created_at DESC,b.id DESC LIMIT ?").bind(&ids).bind(limit).fetch_all(&mut *tx).await?;
    let activity:Vec<_> = rows.into_iter().map(|(id,plan,name,status)|json!({"backup_id":id,"plan_id":plan,"name":name,"catalog_status":status,"stage":live.get(&id),"live_worker":live.contains_key(&id)})).collect();
    let storage_total: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM storage_operations WHERE user_id=? AND state IN ('queued','running')",
    )
    .bind(user)
    .fetch_one(&mut *tx)
    .await?;
    let storage: Vec<(String,String,String,String)> = sqlx::query_as("SELECT id,storage_id,state,stage FROM storage_operations WHERE user_id=? AND state IN ('queued','running') ORDER BY created_at DESC,id DESC LIMIT ?").bind(user).bind(limit).fetch_all(&mut *tx).await?;
    let storage:Vec<_> = storage.into_iter().map(|(id,storage,state,stage)|json!({"operation_id":id,"storage_id":storage,"state":state,"stage":stage})).collect();
    let (failed_backups, failed_copies):(i64,i64) = sqlx::query_as("SELECT count(DISTINCT backup_id),count(*) FROM backup_storage_syncs WHERE sync_status='failed'").fetch_one(&mut *tx).await?;
    let copies:Vec<(i64,i64,Option<String>)> = sqlx::query_as("SELECT s.backup_id,s.storage_id,r.name FROM backup_storage_syncs s LEFT JOIN remote_storages r ON r.id=s.storage_id WHERE s.sync_status='failed' ORDER BY s.id DESC LIMIT ?").bind(limit).fetch_all(&mut *tx).await?;
    let copies:Vec<_> = copies.into_iter().map(|(backup,storage,name)|json!({"backup_id":backup,"storage_id":storage,"storage_name":name,"status":"failed"})).collect();
    tx.commit().await?;
    Ok(
        json!({"observed_at":utc(now),"dataset":"retained_catalog_and_current_process","window":{"after":utc(now-Duration::hours(24)),"before":utc(now),"time_field":"created_at"},
        "plans":{"total":total,"enabled":enabled,"disabled":total-enabled,"manual":manual,"automatic_enabled":automatic_enabled,"automatic_paused":total-manual-automatic_enabled,"without_recorded_run":gaps[0],"without_recorded_completed_run":gaps[1]},
        "backup_activity":{"recorded_pending":pending,"recorded_running":running,"live":live.len(),"total":active,"items":activity},
        "storage_activity":{"total":storage_total,"items":storage},"recorded_copy_failures":{"backups":failed_backups,"total":failed_copies,"items":copies}}),
    )
}
