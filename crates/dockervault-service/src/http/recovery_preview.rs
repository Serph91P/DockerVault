use super::{HttpState, schedules::Cron, security};
use crate::{
    recovery_activation::{ClaimKind, PreparationError},
    recovery_records::RecordValidationError,
    recovery_state::{ValidationContext, validate_state_json},
};
use axum::{
    Json,
    extract::{Request, State},
    http::StatusCode,
    response::{IntoResponse, Response},
};
use dockervault_core::recovery_bundle::{BundleError, decode_json_bundle};
use futures_util::StreamExt;
use serde::Deserialize;
use serde_json::{Value, json};
use std::{collections::BTreeMap, path::Path};
use tokio_util::sync::CancellationToken;

pub(super) const MAX_BUNDLE: usize = 32 * 1024 * 1024;
const MAX_REQUEST: usize = MAX_BUNDLE + 65_536;
pub(super) type Failure = (StatusCode, &'static str);
pub(super) const REQUEST: Failure = (StatusCode::BAD_REQUEST, "Invalid recovery request");
pub(super) const INVALID: Failure = (
    StatusCode::BAD_REQUEST,
    "Invalid or incompatible recovery bundle or password",
);
pub(super) const BUSY: Failure = (
    StatusCode::CONFLICT,
    "Setup or recovery is already in progress",
);
pub(super) const INTERNAL: Failure = (
    StatusCode::INTERNAL_SERVER_ERROR,
    "System recovery failed; inspect setup status before retrying",
);

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Preview {
    bundle: String,
    password: String,
}

#[derive(Deserialize)]
struct RcloneHint {
    rclone: RcloneRemotes,
}
#[derive(Deserialize)]
struct RcloneRemotes {
    remotes: BTreeMap<String, serde::de::IgnoredAny>,
}

pub(super) async fn require_unconfigured(state: &HttpState) -> Result<(), Failure> {
    if state.auth.setup_complete().await.map_err(|_| INTERNAL)? {
        return Err((
            StatusCode::FORBIDDEN,
            "Recovery is only available before initial setup",
        ));
    }
    Ok(())
}

pub(super) fn error((status, detail): Failure) -> Response {
    (status, Json(json!({"detail":detail}))).into_response()
}

pub(super) async fn preview(State(state): State<HttpState>, request: Request) -> Response {
    if let Err(response) = security::rate_limit(&state, &request, "recovery-preview") {
        return *response;
    }
    if let Err(failure) = require_unconfigured(&state).await {
        return error(failure);
    }
    let ctx = state.clone();
    let Ok(work) = state
        .probe_jobs
        .submit_async(move |cancel| validate(ctx, request, cancel))
    else {
        return error(BUSY);
    };
    match work.wait().await.unwrap_or(Err(INTERNAL)) {
        Ok(value) => Json(value).into_response(),
        Err(failure) => error(failure),
    }
}

async fn validate(
    state: HttpState,
    request: Request,
    cancel: CancellationToken,
) -> Result<Value, Failure> {
    let bytes = read_body(request, &cancel).await?;
    let Preview { bundle, password } = serde_json::from_slice(&bytes).map_err(|_| REQUEST)?;
    let result = validate_bundle(&state, bundle, password, &cancel).await?;
    require_unconfigured(&state).await?;
    Ok(result.preview().clone())
}

pub(super) async fn read_body(
    request: Request,
    cancel: &CancellationToken,
) -> Result<zeroize::Zeroizing<Vec<u8>>, Failure> {
    let mut body = request.into_body().into_data_stream();
    let mut bytes = zeroize::Zeroizing::new(Vec::new());
    loop {
        let next = tokio::select! {
            biased;
            _ = cancel.cancelled() => return Err(INTERNAL),
            next = body.next() => next,
        };
        let Some(chunk) = next else { break };
        let chunk = chunk.map_err(|_| REQUEST)?;
        if chunk.len() > MAX_REQUEST - bytes.len() {
            return Err((
                StatusCode::PAYLOAD_TOO_LARGE,
                "Recovery request exceeds size limit",
            ));
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

pub(super) async fn validate_bundle(
    state: &HttpState,
    bundle: String,
    password: String,
    cancel: &CancellationToken,
) -> Result<crate::recovery_state::ValidatedState, Failure> {
    let blocking = state.auth.blocking();
    let claims = state.auth.preparation_claims().clone();
    let owned = cancel.clone();
    let (guard, payload, has_remotes) = blocking
        .submit_blocking(move |physical| {
            if owned.is_cancelled() || physical.is_cancelled() {
                return Err(INTERNAL);
            }

            let password = zeroize::Zeroizing::new(password);
            if bundle.len() > MAX_BUNDLE || password.len() > 1024 {
                return Err(REQUEST);
            }
            let guard = claims
                .try_claim(ClaimKind::Crypto)
                .map_err(|err| match err {
                    PreparationError::Busy => BUSY,
                    _ => INTERNAL,
                })?;
            let payload =
                decode_json_bundle(bundle.as_bytes(), &password).map_err(|err| match err {
                    BundleError::Invalid => INVALID,
                    BundleError::Busy => BUSY,
                    BundleError::Internal => INTERNAL,
                })?;
            let hint: RcloneHint = serde_json::from_str(payload.as_str()).map_err(|_| INVALID)?;
            Ok((guard, payload, !hint.rclone.remotes.is_empty()))
        })
        .map_err(|_| BUSY)?
        .wait()
        .await
        .map_err(|_| INTERNAL)??;
    if cancel.is_cancelled() {
        return Err(INTERNAL);
    }
    let rclone_options = if has_remotes {
        crate::rclone::provider_options(
            state.probe_processes.as_ref().ok_or(INVALID)?,
            blocking,
            Path::new("/usr/local/bin/rclone"),
            cancel.clone(),
        )
        .await
        .map_err(|_| INVALID)?
    } else {
        BTreeMap::new()
    };
    let (now, timezone) = Cron::load(state).map_err(|_| INTERNAL)?.into_clock();
    let context = ValidationContext {
        backup_root: state.config.backup_base_path.clone().into(),
        now,
        timezone,
        rclone_options,
    };
    if cancel.is_cancelled() {
        return Err(INTERNAL);
    }
    // The outer owner retains the file claim while relational validation settles,
    // even when the HTTP waiter disconnects.
    let result = validate_state_json(payload.as_str().as_bytes().to_vec(), context, blocking)
        .await
        .map_err(|e| match e {
            RecordValidationError::Invalid => INVALID,
            RecordValidationError::Cancelled | RecordValidationError::Owner => INTERNAL,
        })?;
    drop(guard);
    Ok(result)
}
