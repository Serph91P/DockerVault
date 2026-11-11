use super::{HttpState, security};
use crate::auth::{AuthError, AuthSession, SessionHash, User};
use axum::{
    Json,
    body::to_bytes,
    extract::{Request, State},
    http::{HeaderMap, HeaderValue, StatusCode},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use serde_json::{Value, json};

pub(super) fn error(error: AuthError) -> Response {
    let (status, detail) = match error {
        AuthError::Preparation(crate::recovery_activation::PreparationError::Busy) => {
            (400, "Setup is already in progress")
        }
        AuthError::Preparation(crate::recovery_activation::PreparationError::InvalidJournal) => {
            (400, "Recovery preparation journal needs repair")
        }
        AuthError::Preparation(crate::recovery_activation::PreparationError::UnsafeGeneration) => {
            (400, "Unsafe recovery generation")
        }
        AuthError::Preparation(
            crate::recovery_activation::PreparationError::UnsafeKeyPreparation,
        ) => (400, "Unsafe instance key preparation"),
        AuthError::Preparation(crate::recovery_activation::PreparationError::Io) => {
            (500, "Internal server error")
        }
        AuthError::AlreadySetup => (400, "Setup already completed. Use login instead."),
        AuthError::SetupNotComplete => (
            400,
            "Setup not complete. Please create an admin account first.",
        ),
        AuthError::PasswordsMismatch => (400, "Passwords do not match"),
        AuthError::InvalidCredentials => (401, "Invalid username or password"),
        AuthError::CurrentPasswordIncorrect => (401, "Current password is incorrect"),
        AuthError::NewPasswordsMismatch => (400, "New passwords do not match"),
        AuthError::UserNotFound => (404, "User not found"),
        AuthError::Busy | AuthError::Cancelled => (503, "Service unavailable"),
        AuthError::Crypto | AuthError::Database => (500, "Internal server error"),
    };
    (
        StatusCode::from_u16(status).expect("constant status code"),
        Json(json!({"detail":detail})),
    )
        .into_response()
}
fn validation(errors: Vec<Value>) -> Box<Response> {
    Box::new(
        (
            StatusCode::UNPROCESSABLE_ENTITY,
            Json(json!({"detail":"Validation error","errors":errors})),
        )
            .into_response(),
    )
}
async fn body(
    request: &mut Request,
    fields: &[(&str, usize, usize)],
) -> Result<Vec<String>, Box<Response>> {
    let raw = to_bytes(std::mem::take(request.body_mut()), 2 * 1024 * 1024)
        .await
        .map_err(|_| {
            Box::new(
                (
                    StatusCode::PAYLOAD_TOO_LARGE,
                    Json(json!({"detail":"Request body too large"})),
                )
                    .into_response(),
            )
        })?;
    if raw.is_empty() {
        return Err(validation(vec![
            json!({"field":"body","message":"Field required"}),
        ]));
    }
    let content_type = request
        .headers()
        .get("content-type")
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.split(';').next())
        .map(|value| value.trim().to_ascii_lowercase());
    if !content_type.as_deref().is_some_and(|value| {
        value == "application/json" || value.starts_with("application/") && value.ends_with("+json")
    }) {
        return Err(validation(vec![
            json!({"field":"body","message":"Input should be a valid dictionary or object to extract fields from"}),
        ]));
    }
    let value: Value = serde_json::from_slice(&raw).map_err(|parse| {
        let prefix = raw
            .split(|byte| *byte == b'\n')
            .take(parse.line().saturating_sub(1))
            .map(|line| line.len() + 1)
            .sum::<usize>();
        let position = prefix + parse.column().saturating_sub(usize::from(!parse.is_eof()));
        validation(vec![
            json!({"field":format!("body.{position}"),"message":"JSON decode error"}),
        ])
    })?;
    let Some(object) = value.as_object() else {
        return Err(validation(vec![
            json!({"field":"body","message":if value.is_null() {"Field required"}else{"Input should be a valid dictionary or object to extract fields from"}}),
        ]));
    };
    let mut errors = Vec::new();
    let mut values = Vec::new();
    for &(field, min, max) in fields {
        let message = match object.get(field) {
            None => Some("Field required".to_owned()),
            Some(Value::String(value)) => {
                let length = value.chars().count();
                if length < min {
                    Some(format!(
                        "String should have at least {min} {}",
                        if min == 1 { "character" } else { "characters" }
                    ))
                } else if length > max {
                    Some(format!("String should have at most {max} characters"))
                } else {
                    values.push(value.clone());
                    None
                }
            }
            Some(_) => Some("Input should be a valid string".to_owned()),
        };
        if let Some(message) = message {
            errors.push(json!({"field":format!("body.{field}"),"message":message}));
        }
    }
    if errors.is_empty() {
        Ok(values)
    } else {
        Err(validation(errors))
    }
}
async fn required_user(state: &HttpState, headers: &HeaderMap) -> Result<User, Box<Response>> {
    let token = security::bearer(headers, false).or_else(|| security::cookie(headers));
    let Some(token) = token else {
        return Err(Box::new(
            (
                StatusCode::UNAUTHORIZED,
                Json(json!({"detail":"Not authenticated"})),
            )
                .into_response(),
        ));
    };
    state
        .auth
        .current(&SessionHash::from_token(&token))
        .await
        .map_err(|failure| Box::new(error(failure)))?
        .ok_or_else(|| {
            Box::new(
                (
                    StatusCode::UNAUTHORIZED,
                    Json(json!({"detail":"Invalid or expired session"})),
                )
                    .into_response(),
            )
        })
}
fn session_response(
    state: &HttpState,
    session: AuthSession,
    message: &str,
    include_user: bool,
) -> Response {
    let cookie = format!(
        "session_token={}; HttpOnly; Max-Age=604800; Path=/; SameSite=lax{}",
        session.token.expose(),
        if state.config.cookie_secure {
            "; Secure"
        } else {
            ""
        }
    );
    let mut response = Json(if include_user {
        json!({"message":message,"user":session.user})
    } else {
        json!({"message":message})
    })
    .into_response();
    response.headers_mut().insert(
        "set-cookie",
        HeaderValue::from_str(&cookie).expect("URL-safe generated token"),
    );
    response
}
pub(super) async fn status(State(state): State<HttpState>, request: Request) -> Response {
    let setup = match state.auth.setup_complete().await {
        Ok(setup) => setup,
        Err(failure) => return error(failure),
    };
    let user = if setup {
        if let Some(token) = security::cookie(request.headers()) {
            match state.auth.current(&SessionHash::from_token(&token)).await {
                Ok(user) => user,
                Err(failure) => return error(failure),
            }
        } else {
            None
        }
    } else {
        None
    };
    Json(json!({"authenticated":user.is_some(),"setup_complete":setup,"user":user})).into_response()
}
pub(super) async fn setup(State(state): State<HttpState>, mut request: Request) -> Response {
    let values = match body(
        &mut request,
        &[
            ("username", 3, 255),
            ("password", 8, 128),
            ("confirm_password", 8, 128),
        ],
    )
    .await
    {
        Ok(values) => values,
        Err(error) => return *error,
    };
    if let Err(response) = security::rate_limit(&state, &request, "setup") {
        return *response;
    }
    match state.auth.setup(&values[0], &values[1], &values[2]).await {
        Ok(session) => session_response(&state, session, "Setup completed successfully", true),
        Err(failure) => error(failure),
    }
}
pub(super) async fn login(State(state): State<HttpState>, mut request: Request) -> Response {
    let values = match body(&mut request, &[("username", 3, 255), ("password", 8, 128)]).await {
        Ok(values) => values,
        Err(error) => return *error,
    };
    if let Err(response) = security::rate_limit(&state, &request, "login") {
        return *response;
    }
    match state.auth.login(&values[0], &values[1]).await {
        Ok(session) => session_response(&state, session, "Login successful", true),
        Err(failure) => error(failure),
    }
}
pub(super) async fn logout(State(state): State<HttpState>, request: Request) -> Response {
    if let Err(failure) = state.auth.logout(security::cookie(request.headers())).await {
        return error(failure);
    }
    let cookie = format!(
        "session_token=\"\"; expires={}; HttpOnly; Max-Age=0; Path=/; SameSite=lax{}",
        Utc::now().format("%a, %d %b %Y %H:%M:%S GMT"),
        if state.config.cookie_secure {
            "; Secure"
        } else {
            ""
        }
    );
    let mut response = Json(json!({"message":"Logged out successfully"})).into_response();
    response.headers_mut().insert(
        "set-cookie",
        HeaderValue::from_str(&cookie).expect("fixed ASCII cookie attributes"),
    );
    response
}
pub(super) async fn me(State(state): State<HttpState>, request: Request) -> Response {
    match required_user(&state, request.headers()).await {
        Ok(user) => Json(user).into_response(),
        Err(response) => *response,
    }
}
pub(super) async fn change_password(
    State(state): State<HttpState>,
    mut request: Request,
) -> Response {
    let user = match required_user(&state, request.headers()).await {
        Ok(user) => user,
        Err(response) => return *response,
    };
    let values = match body(
        &mut request,
        &[
            ("current_password", 1, 128),
            ("new_password", 8, 128),
            ("confirm_password", 8, 128),
        ],
    )
    .await
    {
        Ok(values) => values,
        Err(error) => return *error,
    };
    if let Err(response) = security::rate_limit(&state, &request, "change-password") {
        return *response;
    }
    match state
        .auth
        .change_password(user.id, &values[0], &values[1], &values[2])
        .await
    {
        Ok(session) => session_response(&state, session, "Password changed successfully", false),
        Err(failure) => error(failure),
    }
}
