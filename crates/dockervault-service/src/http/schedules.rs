use super::{HttpState, auth};
use axum::{
    Json,
    extract::{Path, Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use dockervault_core::{
    cron::{CronError, Schedule, TimeZone},
    timestamp::Timestamp,
};
use serde_json::{Value, json};
use sqlx::{Row, sqlite::SqliteRow};
use std::{collections::BTreeMap, path::Path as FsPath, time::Duration};
use strum::EnumMessage;
use time::{UtcDateTime, format_description::well_known::Rfc3339};

const CONTRACT: &str = "Five fields: minute hour day month weekday. Monday=0, Sunday=6; prefer mon..sun. Day-of-month AND weekday must both match. Times use the server timezone and native APScheduler wall-clock/DST behavior; macros are not supported.";

#[cfg(test)]
mod maintenance_tests {
    use super::*;

    #[test]
    fn estimate_wall_end_matches_python_across_gap_and_fold() {
        let zone =
            TimeZone::from_tz_data(&std::fs::read("/usr/share/zoneinfo/Europe/Berlin").unwrap())
                .unwrap();
        for (start, fold, seconds, expected) in [
            (
                "2025-03-30T01:59:00+01:00",
                false,
                120,
                "2025-03-30T02:01:00+01:00",
            ),
            (
                "2025-10-26T02:30:00+01:00",
                true,
                60,
                "2025-10-26T02:31:00+02:00",
            ),
        ] {
            let fire = dockervault_core::cron::FireTime {
                timestamp: Timestamp::parse(start).unwrap(),
                fold,
            };
            assert_eq!(fire.wall_end(seconds, &zone).unwrap().api(), expected);
        }
    }

    #[test]
    fn daily_retention_and_hourly_session_cleanup_follow_server_wall_time() {
        let start = Timestamp::parse("2025-03-30T00:30:00Z").unwrap();
        let zone =
            TimeZone::from_tz_data(&std::fs::read("/usr/share/zoneinfo/Europe/Berlin").unwrap())
                .unwrap();
        let mut jobs = maintenance_jobs(start, &zone);
        assert_eq!(jobs[0].next.timestamp.api(), "2025-03-30T03:00:00+02:00");
        assert_eq!(jobs[1].next.timestamp.api(), "2025-03-30T03:15:00+02:00");
        let retention_now = Timestamp::parse("2025-03-30T01:00:00.100Z").unwrap();
        assert!(jobs[0].take_due(retention_now, &zone));
        assert!(!jobs[0].take_due(retention_now, &zone));
        assert_eq!(jobs[0].next.timestamp.api(), "2025-03-31T03:00:00+02:00");
        assert!(jobs[1].take_due(Timestamp::parse("2025-03-30T01:15:00.100Z").unwrap(), &zone));
    }
}

#[derive(Default)]
pub(super) struct LiveScheduler {
    jobs: tokio::sync::Mutex<BTreeMap<i64, LiveJob>>,
    worker: tokio::sync::Mutex<Option<tokio::task::JoinHandle<()>>>,
}

struct LiveJob {
    name: String,
    expression: String,
    schedule: Schedule,
    next: dockervault_core::cron::FireTime,
}

struct MaintenanceJob {
    schedule: Schedule,
    next: dockervault_core::cron::FireTime,
}

impl MaintenanceJob {
    fn take_due(&mut self, now: Timestamp, zone: &TimeZone) -> bool {
        let Some((now_us, due_us)) = now.unix_micros().zip(self.next.timestamp.unix_micros())
        else {
            return false;
        };
        if now_us < due_us {
            return false;
        }
        let next = if now_us == due_us {
            self.schedule.next_after_fire(self.next, zone, &|| false)
        } else {
            self.schedule.next(now, zone, &|| false)
        };
        let Ok(next) = next else {
            return false;
        };
        self.next = next;
        now_us - due_us <= 1_000_000
    }
}

fn maintenance_jobs(now: Timestamp, zone: &TimeZone) -> [MaintenanceJob; 2] {
    ["0 3 * * *", "15 * * * *"].map(|expression| {
        let schedule = Schedule::parse(expression).expect("valid maintenance schedule");
        let next = schedule
            .next(now, zone, &|| false)
            .expect("maintenance schedule recurs");
        MaintenanceJob { schedule, next }
    })
}

const EFFECTIVE: &str = "SELECT t.id,t.name,CASE WHEN s.id IS NULL THEN t.schedule_cron WHEN s.enabled=1 THEN s.cron_expression END
    FROM backup_targets t LEFT JOIN schedules s ON s.id=t.schedule_id
    WHERE t.enabled=1 AND t.removed_at IS NULL AND (?1 IS NULL OR t.id=?1) ORDER BY t.id";

async fn projected(
    state: &HttpState,
    target: Option<i64>,
) -> Result<Vec<(i64, String, Option<String>)>, Failure> {
    let pool = state.auth.pool();
    let mut tx = pool.begin().await.map_err(db_error)?;
    let rows = if crate::recovery_activation::recovery_pending_in(&mut *tx)
        .await
        .map_err(|_| internal())?
    {
        Vec::new()
    } else {
        sqlx::query_as(EFFECTIVE)
            .bind(target)
            .fetch_all(&mut *tx)
            .await
            .map_err(db_error)?
    };
    tx.commit().await.map_err(db_error)?;
    Ok(rows)
}

pub(super) async fn reconcile(state: &HttpState) -> Result<(), Failure> {
    let failed = || {
        failure(
            StatusCode::SERVICE_UNAVAILABLE,
            "Configuration saved; scheduler reconciliation failed. Retry POST /api/v1/schedules/reload.",
        )
    };
    // Serialize projection and dispatch. Read the committed snapshot only after
    // acquiring this lock so a slower reload cannot reinstall stale jobs.
    let mut jobs = state.scheduler.jobs.lock().await;
    let rows = projected(state, None).await.map_err(|_| failed())?;
    let state_for_cron = state.clone();
    let (next, errors) = tokio::task::spawn_blocking(move || {
        let cron = Cron::load(&state_for_cron)?;
        let mut next = BTreeMap::new();
        let mut errors = false;
        for (id, name, expression) in rows {
            let Some(expression) = expression.filter(|v| !v.is_empty()) else {
                continue;
            };
            let job = Schedule::parse(&expression).and_then(|schedule| {
                let run = schedule.next(cron.now, &cron.zone, &|| cron.cancel.is_cancelled())?;
                Ok(LiveJob {
                    name,
                    expression,
                    schedule,
                    next: run,
                })
            });
            match job {
                Ok(job) => {
                    next.insert(id, job);
                }
                Err(error) => {
                    tracing::warn!(target_id=id,%error,"Could not reconcile scheduled job");
                    errors = true;
                }
            }
        }
        Ok::<_, Failure>((next, errors))
    })
    .await
    .map_err(|_| failed())?
    .map_err(|_| failed())?;
    let mut next = next;
    for (id, job) in &mut next {
        if let Some(old) = jobs.get(id).filter(|old| old.expression == job.expression) {
            job.next = old.next;
        }
    }
    *jobs = next;
    if errors { Err(failed()) } else { Ok(()) }
}

pub(super) async fn start(state: &HttpState) {
    let mut worker = state.scheduler.worker.lock().await;
    if worker.is_some() || state.draining.is_cancelled() {
        return;
    }
    if reconcile(state).await.is_err() {
        tracing::warn!(
            "Startup schedule reconciliation incomplete; retry POST /api/v1/schedules/reload"
        );
    }
    let state = state.clone();
    *worker = Some(tokio::spawn(async move {
        let mut recovery = None;
        let mut maintenance = Cron::load(&state).ok().map(|cron| {
            let jobs = maintenance_jobs(cron.now, &cron.zone);
            (cron.zone, jobs)
        });
        let mut interval = tokio::time::interval(Duration::from_secs(1));
        interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        loop {
            tokio::select! {
                biased;
                _ = state.draining.cancelled() => break,
                _ = interval.tick() => {
                    refresh_recovery_projection(&state, &mut recovery).await;
                    dispatch_due(&state).await;
                    if let Some((zone, [retention, sessions])) = &mut maintenance {
                        let now = now();
                        if retention.take_due(now, zone) {
                            super::retention::start_sweep(&state);
                        }
                        if sessions.take_due(now, zone)
                            && let Err(error) = sqlx::query("DELETE FROM sessions WHERE expires_at<?").bind(now.sql()).execute(state.auth.pool()).await {
                            tracing::warn!(%error,"Expired session cleanup did not finish");
                        }
                    }
                }
            }
        }
    }));
}

pub(super) async fn stop(state: &HttpState) {
    if let Some(worker) = state.scheduler.worker.lock().await.take() {
        let _ = worker.await;
    }
}

async fn refresh_recovery_projection(state: &HttpState, projected: &mut Option<Option<String>>) {
    let marker: Result<Option<Option<String>>, _> =
        sqlx::query_scalar("SELECT value FROM app_settings WHERE key='recovery_activation'")
            .fetch_optional(state.auth.pool())
            .await;
    let Ok(marker) = marker else {
        return;
    };
    let marker = marker.flatten();
    if projected.as_ref() != Some(&marker) && reconcile(state).await.is_ok() {
        *projected = Some(marker);
    }
}

async fn dispatch_due(state: &HttpState) {
    let mut jobs = state.scheduler.jobs.lock().await;
    let now = now().unix_micros().expect("UTC timestamp");
    if !jobs.values().any(|job| {
        job.next
            .timestamp
            .unix_micros()
            .is_some_and(|run| run <= now)
    }) {
        return;
    }
    let Ok(cron) = Cron::load(state) else {
        return;
    };
    let mut due = Vec::new();
    jobs.retain(|id, job| {
        let Some(run) = job.next.timestamp.unix_micros() else {
            return false;
        };
        if run > now {
            return true;
        }
        // APScheduler's default misfire grace is one second.
        if now - run <= 1_000_000 {
            due.push((*id, job.expression.clone()));
        }
        match job
            .schedule
            .next(cron.now, &cron.zone, &|| cron.cancel.is_cancelled())
        {
            Ok(next) => {
                job.next = next;
                true
            }
            Err(_) => false,
        }
    });
    for (id, expression) in due {
        if state.draining.is_cancelled() {
            break;
        }
        if let Ok(target) = projected(state, Some(id)).await
            && target.first().and_then(|(_, _, value)| value.as_deref())
                == Some(expression.as_str())
        {
            let response = super::backup_submit::trigger_scheduled(state, id).await;
            if !response.status().is_success() {
                tracing::warn!(target_id=id,status=%response.status(),"Scheduled backup was not admitted");
            }
        }
    }
}

pub(super) async fn jobs(State(state): State<HttpState>) -> Json<Value> {
    let jobs = state.scheduler.jobs.lock().await;
    let mut jobs: Vec<_> = jobs.iter().collect();
    jobs.sort_by_key(|(id, job)| (job.next.timestamp.unix_micros(), **id));
    Json(Value::Array(jobs.into_iter().map(|(id,job)| json!({"id":format!("backup_target_{id}"),"name":format!("Backup: {}",job.name),"next_run":job.next.timestamp})).collect()))
}

pub(super) async fn reload(State(state): State<HttpState>) -> Result<Json<Value>, Failure> {
    reconcile(&state).await?;
    Ok(Json(json!({"status":"reloaded"})))
}

pub(super) async fn estimate(
    State(state): State<HttpState>,
    mut request: Request,
) -> Result<Json<Value>, Failure> {
    let body = auth::json_body(&state, &mut request)
        .await
        .map_err(Failure)?;
    let id = match body.get("target_id") {
        None => Err("Field required"),
        Some(Value::String(v)) => super::operations::query_integer(v)
            .ok_or("Input should be a valid integer, unable to parse string as an integer"),
        Some(Value::Bool(v)) => Ok(i64::from(*v)),
        Some(Value::Number(v)) => v
            .as_i64()
            .or_else(|| {
                v.as_f64()
                    .filter(|n| {
                        n.fract() == 0.0 && *n >= i64::MIN as f64 && *n < -(i64::MIN as f64)
                    })
                    .map(|n| n as i64)
            })
            .ok_or("Input should be a valid integer, got a number with a fractional part"),
        _ => Err("Input should be a valid integer"),
    };
    let expression = body.get("cron_expression").and_then(Value::as_str);
    let mut errors = vec![];
    if let Err(message) = id {
        errors.push(json!({"field":"body.target_id","message":message}));
    }
    if expression.is_none() {
        errors.push(json!({"field":"body.cron_expression","message":if body.get("cron_expression").is_some() {"Input should be a valid string"} else {"Field required"}}));
    }
    if !errors.is_empty() {
        return Err(Failure(auth::validation(errors)));
    }
    let id = id.expect("validated id");
    let expression = expression.expect("validated expression").to_owned();
    let row: Option<(Option<f64>,)> = sqlx::query_as("SELECT (SELECT AVG(duration_seconds) FROM (SELECT duration_seconds FROM backups WHERE target_id=?1 AND status='COMPLETED' AND duration_seconds IS NOT NULL ORDER BY created_at DESC LIMIT 5)) FROM backup_targets WHERE id=?1")
        .bind(id).fetch_optional(state.auth.pool()).await.map_err(db_error)?;
    let (average,) = row.ok_or_else(|| failure(StatusCode::NOT_FOUND, "Target not found"))?;
    let duration = average.map_or(60, |value| (value * 1.2) as i64);
    let jobs = {
        let jobs = state.scheduler.jobs.lock().await;
        let mut jobs: Vec<_> = jobs
            .iter()
            .map(|(&id, job)| (id, job.name.clone(), job.next.timestamp))
            .collect();
        jobs.sort_by_key(|(id, _, time)| (time.unix_micros(), *id));
        jobs
    };
    blocking(state, move |_, cron| {
        let start = Schedule::parse(&expression).and_then(|s| s.next(cron.now, &cron.zone, &|| cron.cancel.is_cancelled())).map_err(|_| internal())?;
        let end = start.wall_end(duration, &cron.zone).map_err(|_| internal())?;
        // Python compares aware datetimes in the same ZoneInfo by wall clock, including folds.
        let start_wall = start.timestamp.sql();
        let end_wall = end.sql();
        let conflicts: Vec<_> = jobs.into_iter().filter_map(|(id,name,time)| {
            let wall = time.sql();
            (start_wall <= wall && wall <= end_wall).then(|| json!({"job_id":format!("backup_target_{id}"),"job_name":format!("Backup: {name}"),"scheduled_time":time}))
        }).collect();
        Ok(json!({"start_time":start.timestamp,"estimated_end_time":end,"duration_seconds":duration,"conflicts":conflicts}))
    }).await
}

pub(super) struct Failure(Box<Response>);

impl IntoResponse for Failure {
    fn into_response(self) -> Response {
        *self.0
    }
}

fn failure(status: StatusCode, detail: impl Into<String>) -> Failure {
    Failure(Box::new(
        (status, Json(json!({"detail": detail.into()}))).into_response(),
    ))
}
fn internal() -> Failure {
    failure(StatusCode::INTERNAL_SERVER_ERROR, "Internal server error")
}
fn now() -> Timestamp {
    Timestamp::parse(
        &UtcDateTime::now()
            .format(&Rfc3339)
            .expect("current UTC date"),
    )
    .expect("valid current UTC timestamp")
}

pub(super) struct Cron {
    zone: TimeZone,
    now: Timestamp,
    cancel: tokio_util::sync::CancellationToken,
}
impl Cron {
    pub(super) fn into_clock(self) -> (Timestamp, TimeZone) {
        (self.now, self.zone)
    }

    pub(super) fn load(state: &HttpState) -> Result<Self, Failure> {
        let name = &state.config.tz;
        if name.is_empty()
            || !FsPath::new(name)
                .components()
                .all(|part| matches!(part, std::path::Component::Normal(_)))
        {
            return Err(internal());
        }
        let path = FsPath::new("/usr/share/zoneinfo").join(name);
        let zone = std::fs::read(path)
            .ok()
            .and_then(|data| TimeZone::from_tz_data(&data).ok())
            .ok_or_else(internal)?;
        Ok(Self {
            zone,
            now: now(),
            cancel: state.draining.clone(),
        })
    }
    fn next(&self, schedule: &Schedule) -> Result<String, CronError> {
        schedule
            .next(self.now, &self.zone, &|| self.cancel.is_cancelled())
            .map(|run| run.timestamp.api())
    }
    pub(super) fn inspect(&self, expression: &str) -> (Option<String>, Vec<String>) {
        if expression.is_empty() {
            return (None, vec![]);
        }
        let run = match Schedule::parse(expression).and_then(|schedule| self.next(&schedule)) {
            Ok(run) => run,
            Err(e) => {
                return (
                    None,
                    vec![format!(
                        "Invalid stored schedule; cannot run until corrected: {e}. {CONTRACT}"
                    )],
                );
            }
        };
        let parts: Vec<_> = expression
            .split(|c: char| c.is_whitespace() || matches!(c, '\u{1c}'..='\u{1f}'))
            .filter(|s| !s.is_empty())
            .collect();
        let mut warnings = vec![];
        if parts[4] != "*" && parts[4].chars().any(char::is_numeric) {
            warnings.push("Numeric weekdays retain existing execution: Monday=0, Sunday=6 (not Unix). Review intent; prefer mon..sun. No automatic conversion was performed.".into());
        }
        if parts[2] != "*" && parts[4] != "*" {
            warnings.push("Day-of-month AND weekday must both match, not Unix OR. Existing execution is unchanged; review intent.".into());
        }
        (Some(run), warnings)
    }
}

fn parse_base(value: &Value) -> Result<Option<Timestamp>, String> {
    use speedate::{
        Date, DateTime, DateTimeConfig, MicrosecondsPrecisionOverflowBehavior, TimeConfig,
        TimestampUnit,
    };
    if value.is_null() {
        return Ok(None);
    }
    let number;
    let raw = if let Some(text) = value.as_str() {
        text
    } else if value.is_number() {
        number = value.to_string();
        &number
    } else {
        return Err("Input should be a valid datetime".into());
    };
    let config = DateTimeConfig {
        timestamp_unit: TimestampUnit::Infer,
        time_config: TimeConfig {
            microseconds_precision_overflow_behavior:
                MicrosecondsPrecisionOverflowBehavior::Truncate,
            unix_timestamp_offset: Some(0),
        },
    };
    match DateTime::parse_bytes_with_config(raw.as_bytes(), &config) {
        Ok(dt) if dt.time.tz_offset.is_some() => Timestamp::parse(&dt.to_string())
            .map(Some)
            .map_err(|_| "Input should be a valid datetime".into()),
        Ok(_) => Err("Input should have timezone info".into()),
        Err(_) if Date::parse_str(raw).is_ok() => Err("Input should have timezone info".into()),
        Err(err) => {
            let reason = if value.is_string() {
                Date::parse_str(raw).err().unwrap_or(err)
            } else {
                err
            };
            let reason = reason
                .get_documentation()
                .unwrap_or("invalid date or datetime");
            Err(format!(
                "Input should be a valid datetime or date, {reason}"
            ))
        }
    }
}

fn integer_string(input: &str) -> Option<f64> {
    let input = input.trim();
    let unsigned = input.strip_prefix(['-', '+']).unwrap_or(input);
    let digits = match unsigned.split_once('.') {
        Some((integer, fraction))
            if !fraction.is_empty() && fraction.bytes().all(|b| b == b'0') =>
        {
            integer
        }
        Some(_) => return None,
        None => unsigned,
    };
    let mut value = 0_u8;
    for group in digits.split('_') {
        if group.is_empty() || !group.bytes().all(|b| b.is_ascii_digit()) {
            return None;
        }
        for digit in group.bytes() {
            value = (value * 10 + digit - b'0').min(11);
        }
    }
    let value = f64::from(value);
    Some(if input.starts_with('-') {
        -value
    } else {
        value
    })
}

fn count(value: Option<&Value>) -> Result<usize, &'static str> {
    let Some(value) = value else {
        return Ok(3);
    };
    let number = match value {
        Value::Bool(v) => f64::from(u8::from(*v)),
        Value::Number(n) => n.as_f64().unwrap_or(f64::INFINITY),
        Value::String(s) => integer_string(s)
            .ok_or("Input should be a valid integer, unable to parse string as an integer")?,
        _ => return Err("Input should be a valid integer"),
    };
    if !number.is_finite() || number.fract() != 0.0 {
        return Err("Input should be a valid integer, got a number with a fractional part");
    }
    if number < 1.0 {
        return Err("Input should be greater than or equal to 1");
    }
    if number > 10.0 {
        return Err("Input should be less than or equal to 10");
    }
    Ok(number as usize)
}

pub(super) async fn preview(
    State(state): State<HttpState>,
    mut request: Request,
) -> Result<Json<Value>, Failure> {
    let body = auth::json_body(&state, &mut request)
        .await
        .map_err(Failure)?;
    let mut errors = vec![];
    let expression = body.get("cron_expression").and_then(Value::as_str);
    if expression.is_none() {
        errors.push(json!({"field":"body.cron_expression","message":if body.get("cron_expression").is_none() { "Field required" } else { "Input should be a valid string" }}));
    }
    let base = body.get("base_time").map(parse_base).transpose();
    if let Err(message) = &base {
        errors.push(json!({"field":"body.base_time","message":message}));
    }
    let count = count(body.get("count"));
    if let Err(message) = &count {
        errors.push(json!({"field":"body.count","message":message}));
    }
    if !errors.is_empty() {
        return Err(Failure(auth::validation(errors)));
    }
    let expression = expression.expect("validated expression").to_owned();
    let count = count.expect("validated count");
    let base = base.expect("validated base").flatten();
    blocking(state, move |state, cron| {
        let invalid = |e: CronError| if matches!(e, CronError::Cancelled) {
            failure(StatusCode::SERVICE_UNAVAILABLE, "Service shutting down")
        } else { failure(StatusCode::BAD_REQUEST, format!("Invalid cron expression: {e}")) };
        let cancel = || cron.cancel.is_cancelled();
        let schedule = Schedule::parse(&expression).map_err(invalid)?;
        let mut run = schedule.next(base.unwrap_or(cron.now), &cron.zone, &cancel).map_err(invalid)?;
        let mut runs = vec![run.timestamp.api()];
        for _ in 1..count {
            run = schedule.next_after_fire(run, &cron.zone, &cancel).map_err(invalid)?;
            runs.push(run.timestamp.api());
        }
        Ok(json!({"next_runs":runs,"timezone":state.config.tz,"contract":CONTRACT,"warnings":cron.inspect(&expression).1}))
    }).await
}

async fn blocking(
    state: HttpState,
    compute: impl FnOnce(HttpState, Cron) -> Result<Value, Failure> + Send + 'static,
) -> Result<Json<Value>, Failure> {
    tokio::task::spawn_blocking(move || {
        let cron = Cron::load(&state)?;
        compute(state, cron).map(Json)
    })
    .await
    .map_err(|_| internal())?
}

fn schedule(row: &SqliteRow, cron: &Cron) -> Result<Value, sqlx::Error> {
    let expression: String = row.try_get("cron_expression")?;
    let enabled: bool = row.try_get("enabled")?;
    let (next, warnings) = cron.inspect(&expression);
    let timestamp = |key| -> Result<_, sqlx::Error> {
        let raw: String = row.try_get(key)?;
        Timestamp::parse(&raw)
            .map(|dt| dt.api())
            .map_err(|e| sqlx::Error::Protocol(e.to_string()))
    };
    Ok(
        json!({"id":row.try_get::<i64,_>("id")?,"name":row.try_get::<String,_>("name")?,
        "description":row.try_get::<Option<String>,_>("description")?,"cron_expression":expression,
        "enabled":enabled,"target_count":row.try_get::<i64,_>("target_count")?,
        "next_run":if enabled { next } else { None },"cron_warnings":warnings,
        "created_at":timestamp("created_at")?,"updated_at":timestamp("updated_at")?}),
    )
}

const SCHEDULES: &str = "SELECT s.*, (SELECT count(*) FROM backup_targets t WHERE t.schedule_id=s.id AND t.removed_at IS NULL) AS target_count FROM schedules s WHERE (?1 IS NULL OR s.id=?1) ORDER BY s.id";
fn db_error(_: sqlx::Error) -> Failure {
    internal()
}

pub(super) async fn list(State(state): State<HttpState>) -> Result<Json<Value>, Failure> {
    let rows = sqlx::query(SCHEDULES)
        .bind(None::<i64>)
        .fetch_all(state.auth.pool())
        .await
        .map_err(db_error)?;
    blocking(state, move |_, cron| {
        rows.iter()
            .map(|row| schedule(row, &cron))
            .collect::<Result<Vec<_>, _>>()
            .map(Value::Array)
            .map_err(db_error)
    })
    .await
}

pub(super) async fn detail(
    State(state): State<HttpState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Failure> {
    let id = id
        .bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| id.parse::<i64>().ok())
        .flatten()
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "Not Found"))?;
    let mut tx = state.auth.pool().begin().await.map_err(db_error)?;
    let row = sqlx::query(SCHEDULES)
        .bind(id)
        .fetch_optional(&mut *tx)
        .await
        .map_err(db_error)?
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "Schedule not found"))?;
    let targets: Vec<(i64,String,String,Option<bool>)> = sqlx::query_as("SELECT id,name,target_type,enabled FROM backup_targets WHERE schedule_id=? AND removed_at IS NULL ORDER BY id").bind(id).fetch_all(&mut *tx).await.map_err(db_error)?;
    tx.commit().await.map_err(db_error)?;
    blocking(state, move |_, cron| {
        let mut result = schedule(&row, &cron).map_err(db_error)?;
        result["targets"] = Value::Array(targets.into_iter().map(|(id,name,kind,enabled)| json!({"id":id,"name":name,"target_type":kind,"enabled":enabled})).collect());
        Ok(result)
    }).await
}

pub(super) async fn inventory(State(state): State<HttpState>) -> Result<Json<Value>, Failure> {
    let rows: Vec<(String, i64, String, Option<String>)> =
        sqlx::query_as("SELECT 'schedule',id,name,cron_expression FROM schedules ORDER BY id")
            .fetch_all(state.auth.pool())
            .await
            .map_err(db_error)?;
    let targets: Vec<(String,i64,String,Option<String>)> = sqlx::query_as("SELECT 'target_legacy',id,name,schedule_cron FROM backup_targets WHERE schedule_cron IS NOT NULL ORDER BY id").fetch_all(state.auth.pool()).await.map_err(db_error)?;
    blocking(state, move |state, cron| {
        let items: Vec<_> = rows.into_iter().chain(targets).filter_map(|(kind,id,name,expression)| {
            let expression = expression?;
            let warnings = cron.inspect(&expression).1;
            (!warnings.is_empty()).then(|| json!({"kind":kind,"id":id,"name":name,"expression":expression,"warnings":warnings}))
        }).collect();
        Ok(json!({"items":items,"contract":CONTRACT,"timezone":state.config.tz}))
    }).await
}

pub(super) async fn legacy(State(state): State<HttpState>) -> Result<Json<Value>, Failure> {
    let rows: Vec<(i64,String,Option<String>,bool,bool)> = sqlx::query_as("SELECT t.id,t.name,CASE WHEN s.id IS NOT NULL THEN s.cron_expression ELSE t.schedule_cron END, coalesce(t.enabled,0), CASE WHEN s.id IS NOT NULL THEN coalesce(s.enabled,0) ELSE 1 END FROM backup_targets t LEFT JOIN schedules s ON s.id=t.schedule_id WHERE t.removed_at IS NULL AND (t.schedule_id IS NOT NULL OR t.schedule_cron IS NOT NULL) ORDER BY t.id").fetch_all(state.auth.pool()).await.map_err(db_error)?;
    blocking(state, move |_, cron| {
        Ok(Value::Array(rows.into_iter().filter_map(|(id,name,expression,target_enabled,schedule_enabled)| {
            let expression = expression.filter(|s| !s.is_empty())?;
            let enabled = target_enabled && schedule_enabled;
            let next_run = enabled.then(|| Schedule::parse(&expression).and_then(|s| cron.next(&s)).ok()).flatten();
            Some(json!({"id":id,"target_id":id,"target_name":name,"cron_expression":expression,"next_run":next_run,"last_run":null,"enabled":enabled}))
        }).collect()))
    }).await
}

pub(super) async fn trigger_target(
    State(state): State<HttpState>,
    Path(id): Path<String>,
) -> Response {
    let Some(id) = super::operations::query_integer(&id) else {
        return *auth::validation(vec![
            json!({"field":"path.target_id","message":"Input should be a valid integer, unable to parse string as an integer"}),
        ]);
    };
    super::backup_submit::trigger_full(&state, id).await
}

pub(super) async fn trigger_all(
    State(state): State<HttpState>,
    Path(id): Path<String>,
) -> Result<Response, Failure> {
    let Some(id) = super::operations::query_integer(&id) else {
        return Err(Failure(auth::validation(vec![
            json!({"field":"path.schedule_id","message":"Input should be a valid integer, unable to parse string as an integer"}),
        ])));
    };
    let rows: Vec<(Option<i64>, Option<bool>)> = sqlx::query_as("SELECT t.id,t.enabled FROM schedules s LEFT JOIN backup_targets t ON t.schedule_id=s.id AND t.removed_at IS NULL WHERE s.id=? ORDER BY t.id")
        .bind(id).fetch_all(state.auth.pool()).await.map_err(db_error)?;
    if rows.is_empty() {
        return Err(failure(StatusCode::NOT_FOUND, "Schedule not found"));
    }
    let (mut enabled, mut skipped) = (Vec::new(), Vec::new());
    for (target, active) in rows {
        if let Some(target) = target {
            if active == Some(true) {
                enabled.push(target);
            } else {
                skipped.push(target);
            }
        }
    }
    if enabled.is_empty() {
        return Ok(Json(json!({"status":"triggered","schedule_id":id,"triggered_targets":[],"skipped_targets":skipped})).into_response());
    }
    Ok(super::backup_submit::trigger_batch(&state, id, enabled, skipped).await)
}

pub(super) async fn create(
    State(state): State<HttpState>,
    request: Request,
) -> Result<Json<Value>, Failure> {
    save(state, None, request).await
}

pub(super) async fn update(
    State(state): State<HttpState>,
    Path(id): Path<String>,
    request: Request,
) -> Result<Json<Value>, Failure> {
    save(state, Some(schedule_id(&id)?), request).await
}

fn schedule_id(id: &str) -> Result<i64, Failure> {
    id.bytes()
        .all(|b| b.is_ascii_digit())
        .then(|| id.parse().ok())
        .flatten()
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "Not Found"))
}

async fn save(
    state: HttpState,
    id: Option<i64>,
    mut request: Request,
) -> Result<Json<Value>, Failure> {
    let body = auth::json_body(&state, &mut request)
        .await
        .map_err(Failure)?;
    let mut errors = Vec::new();
    let mut text = |key: &str, required: bool| match body.get(key) {
        Some(Value::String(value)) => Some(value.clone()),
        value if !required && value.is_none_or(Value::is_null) => None,
        value => {
            let message = if value.is_none() {
                "Field required"
            } else {
                "Input should be a valid string"
            };
            errors.push(json!({"field":format!("body.{key}"),"message":message}));
            None
        }
    };
    let name = text("name", id.is_none());
    let expression = text("cron_expression", id.is_none());
    let description = text("description", false);
    let enabled = match body.get("enabled") {
        None => id.is_none().then_some(true),
        Some(Value::Null) if id.is_some() => None,
        Some(Value::Bool(value)) => Some(*value),
        Some(value) => {
            let parsed = if let Some(text) = value.as_str() {
                match text.to_ascii_lowercase().as_str() {
                    "1" | "true" | "t" | "yes" | "y" | "on" => Some(true),
                    "0" | "false" | "f" | "no" | "n" | "off" => Some(false),
                    _ => None,
                }
            } else {
                value
                    .as_f64()
                    .filter(|n| *n == 0.0 || *n == 1.0)
                    .map(|n| n == 1.0)
            };
            if parsed.is_none() {
                let message = if value.is_string() || value.is_number() {
                    "Input should be a valid boolean, unable to interpret input"
                } else {
                    "Input should be a valid boolean"
                };
                errors.push(json!({"field":"body.enabled","message":message}));
            }
            parsed
        }
    };
    if !errors.is_empty() {
        return Err(Failure(auth::validation(errors)));
    }
    if let Some(check) = expression.clone() {
        let _ = blocking(state.clone(), move |_, cron| {
            Schedule::parse(&check)
                .and_then(|schedule| cron.next(&schedule))
                .map_err(|e| {
                    failure(
                        StatusCode::BAD_REQUEST,
                        format!("Invalid cron expression: {e}"),
                    )
                })?;
            Ok(Value::Null)
        })
        .await?;
    }
    let mut tx = state
        .auth
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(db_error)?;
    let write = if let Some(id) = id {
        sqlx::query_scalar("UPDATE schedules SET name=coalesce(?,name),cron_expression=coalesce(?,cron_expression),description=coalesce(?,description),enabled=coalesce(?,enabled),updated_at=? WHERE id=? RETURNING id")
            .bind(&name).bind(expression).bind(description).bind(enabled).bind(now().sql()).bind(id)
    } else {
        let stamp = now().sql();
        sqlx::query_scalar("INSERT INTO schedules(name,cron_expression,description,enabled,created_at,updated_at) VALUES (?,?,?,?,?,?) RETURNING id")
            .bind(&name).bind(expression).bind(description).bind(enabled).bind(stamp.clone()).bind(stamp)
    };
    let saved: i64 = write
        .fetch_optional(&mut *tx)
        .await
        .map_err(|e| {
            if e.as_database_error()
                .is_some_and(|e| e.is_unique_violation())
            {
                failure(
                    StatusCode::BAD_REQUEST,
                    format!(
                        "Schedule with name '{}' already exists",
                        name.unwrap_or_default()
                    ),
                )
            } else {
                db_error(e)
            }
        })?
        .ok_or_else(|| failure(StatusCode::NOT_FOUND, "Schedule not found"))?;
    let row = sqlx::query(SCHEDULES)
        .bind(saved)
        .fetch_one(&mut *tx)
        .await
        .map_err(db_error)?;
    tx.commit().await.map_err(db_error)?;
    reconcile(&state).await?;
    blocking(state, move |_, cron| {
        let mut response = schedule(&row, &cron).map_err(db_error)?;
        if id.is_none() {
            response["next_run"] = Value::Null;
        }
        Ok(response)
    })
    .await
}

pub(super) async fn delete(
    State(state): State<HttpState>,
    Path(id): Path<String>,
) -> Result<Json<Value>, Failure> {
    let id = schedule_id(&id)?;
    let mut tx = state
        .auth
        .pool()
        .begin_with("BEGIN IMMEDIATE")
        .await
        .map_err(db_error)?;
    let unlinked = sqlx::query("UPDATE backup_targets SET schedule_id=NULL WHERE schedule_id=?")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?
        .rows_affected();
    let deleted = sqlx::query("DELETE FROM schedules WHERE id=?")
        .bind(id)
        .execute(&mut *tx)
        .await
        .map_err(db_error)?
        .rows_affected();
    if deleted != 1 {
        return Err(failure(StatusCode::NOT_FOUND, "Schedule not found"));
    }
    tx.commit().await.map_err(db_error)?;
    reconcile(&state).await?;
    Ok(Json(
        json!({"status":"deleted","unlinked_targets":unlinked}),
    ))
}

pub(super) async fn help(State(state): State<HttpState>) -> Json<Value> {
    Json(
        json!({"format":"minute hour day month weekday","contract":CONTRACT,"timezone":state.config.tz,
        "examples":[{"expression":"0 2 * * *","description":"Every day at 2:00 AM"},
            {"expression":"0 3 * * sun","description":"Every Sunday at 3:00 AM"},
            {"expression":"0 4 1 * *","description":"First day of every month at 4:00 AM"},
            {"expression":"0 */6 * * *","description":"Every 6 hours"},
            {"expression":"30 1 * * mon-fri","description":"Weekdays at 1:30 AM"}],
        "special":{"*":"Any value","*/n":"Every n units","n-m":"Range from n to m","n,m":"n and m"}}),
    )
}
