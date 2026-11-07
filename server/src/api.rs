//! Conventions of the JSON API under `/api`, which the web client calls.
//!
//! - Bodies are JSON objects with camelCase keys, as are real-time events.
//! - A handler returns `Result<_, ApiError>`. Every error has a JSON body with a snake_case
//!   `error` code that the web client can act on, and nothing else from inside the server: the
//!   cause of a 500 goes only to the log, with the request's correlation ID (MNT-03, UI-06).
//! - Request bodies are read with [`axum::Json`], which requires `Content-Type:
//!   application/json`. Browsers send that type to another site only after a CORS preflight,
//!   which the server never grants, so other sites cannot make a user's browser call the API.
//!   Take the body as `Result<Json<T>, JsonRejection>` and use `?`, so that a malformed body gets
//!   the same kind of answer as other errors. Take path parameters as `Result<Path<T>,
//!   PathRejection>` for the same reason.
//! - Requests that change something are refused when another site started them, whatever their
//!   body ([`crate::http`]).
//!
//! | Status | `error` | When |
//! |---|---|---|
//! | 400 | `malformed` | The body is not the JSON the endpoint expects. |
//! | 401 | `sign_in_failed`, `not_signed_in` | The credentials are wrong, or the request needs a sign-in it doesn't have. |
//! | 403 | `cross_site_request` | Another site started a request that would change something. |
//! | 409 | e.g. `terms_changed` | The request is based on something that has changed. |
//! | 410 | e.g. `link_expired`, `sign_in_ended` | What the request refers to is gone or never existed. |
//! | 422 | `invalid` | Fields don't meet the rules; `fields` says which, and why. |
//! | 428 | e.g. `challenge_required` | Something must be done first; the body says what. |
//! | 429 | `too_many_requests` | Too many requests; `Retry-After` says when to try again. |
//! | 500 | `internal` | Something failed inside the server. |

use std::{collections::BTreeMap, time::Duration};

use axum::{
    Json,
    extract::rejection::{JsonRejection, PathRejection},
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use serde::Serialize;
use serde_json::{Map, Value, json};
use tracing::error;

/// Why a request failed, as the client sees it.
#[derive(Debug)]
pub enum ApiError {
    /// 400: the body is not the JSON the endpoint expects.
    Malformed,
    /// 401: the credentials are wrong, or the request needs a sign-in it doesn't have.
    Unauthorized { code: &'static str },
    /// 403: the request is not allowed, whoever makes it.
    Forbidden { code: &'static str },
    /// 422: some fields don't meet the rules.
    Invalid(FieldErrors),
    /// 409: the request is based on something that has changed.
    Conflict { code: &'static str },
    /// 410: what the request refers to is gone, or never existed.
    Gone { code: &'static str },
    /// 428: something must be done first. `details` are added to the body.
    PreconditionRequired {
        code: &'static str,
        details: Map<String, Value>,
    },
    /// 429: too many requests; the client may try again after `retry_after`.
    TooManyRequests { retry_after: Duration },
    /// 500: something failed inside the server. The cause is only logged.
    Internal(Box<dyn std::error::Error + Send + Sync>),
}

/// The fields of a request that don't meet the rules, by their name in the request.
pub type FieldErrors = BTreeMap<&'static str, FieldError>;

/// Why a field doesn't meet the rules: a snake_case code, such as `required` or `too_short`, and
/// the limit it doesn't meet, if any.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize)]
pub struct FieldError {
    pub code: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<usize>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<usize>,
}

impl FieldError {
    pub fn new(code: &'static str) -> Self {
        Self {
            code,
            min: None,
            max: None,
        }
    }

    pub fn min(code: &'static str, min: usize) -> Self {
        Self {
            min: Some(min),
            ..Self::new(code)
        }
    }

    pub fn max(code: &'static str, max: usize) -> Self {
        Self {
            max: Some(max),
            ..Self::new(code)
        }
    }
}

impl ApiError {
    /// A 500 for `err`, which is logged but never sent.
    pub fn internal(err: impl std::error::Error + Send + Sync + 'static) -> Self {
        Self::Internal(Box::new(err))
    }
}

impl From<JsonRejection> for ApiError {
    fn from(_: JsonRejection) -> Self {
        Self::Malformed
    }
}

impl From<PathRejection> for ApiError {
    fn from(_: PathRejection) -> Self {
        Self::Malformed
    }
}

impl From<sea_orm::DbErr> for ApiError {
    fn from(err: sea_orm::DbErr) -> Self {
        Self::internal(err)
    }
}

impl From<crate::audit::Error> for ApiError {
    fn from(err: crate::audit::Error) -> Self {
        Self::internal(err)
    }
}

impl From<crate::realtime::PublishError> for ApiError {
    fn from(err: crate::realtime::PublishError) -> Self {
        Self::internal(err)
    }
}

impl From<crate::accounts::sign_in_session::RevokeError> for ApiError {
    fn from(err: crate::accounts::sign_in_session::RevokeError) -> Self {
        Self::internal(err)
    }
}

impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let (status, body) = match self {
            Self::Malformed => (StatusCode::BAD_REQUEST, json!({ "error": "malformed" })),
            Self::Invalid(fields) => (
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({ "error": "invalid", "fields": fields }),
            ),
            Self::Unauthorized { code } => (StatusCode::UNAUTHORIZED, json!({ "error": code })),
            Self::Forbidden { code } => (StatusCode::FORBIDDEN, json!({ "error": code })),
            Self::Conflict { code } => (StatusCode::CONFLICT, json!({ "error": code })),
            Self::Gone { code } => (StatusCode::GONE, json!({ "error": code })),
            Self::PreconditionRequired { code, mut details } => {
                details.insert("error".to_owned(), code.into());
                (StatusCode::PRECONDITION_REQUIRED, Value::Object(details))
            }
            Self::TooManyRequests { retry_after } => {
                // Whole seconds, rounded up, so that a client that waits that long is let in.
                let seconds = retry_after.as_secs() + u64::from(retry_after.subsec_nanos() > 0);
                let mut response = (
                    StatusCode::TOO_MANY_REQUESTS,
                    Json(json!({ "error": "too_many_requests" })),
                )
                    .into_response();
                response
                    .headers_mut()
                    .insert(header::RETRY_AFTER, HeaderValue::from(seconds.max(1)));
                return response;
            }
            Self::Internal(err) => {
                // Within the request's span, so the log line carries its correlation ID.
                error!(error = %err, "request failed");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    json!({ "error": "internal" }),
                )
            }
        };
        (status, Json(body)).into_response()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn parts(error: ApiError) -> (StatusCode, Option<String>, Value) {
        let response = error.into_response();
        let status = response.status();
        let retry_after = response
            .headers()
            .get(header::RETRY_AFTER)
            .map(|value| value.to_str().unwrap().to_owned());
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        (status, retry_after, serde_json::from_slice(&bytes).unwrap())
    }

    #[tokio::test]
    async fn errors_have_a_code_and_status() {
        let mut fields = FieldErrors::new();
        fields.insert("password", FieldError::min("too_short", 12));
        fields.insert("email", FieldError::new("invalid"));
        let mut details = Map::new();
        details.insert("challenge".to_owned(), json!({ "difficulty": 18 }));

        for (error, status, body) in [
            (
                ApiError::Malformed,
                StatusCode::BAD_REQUEST,
                json!({ "error": "malformed" }),
            ),
            (
                ApiError::Invalid(fields),
                StatusCode::UNPROCESSABLE_ENTITY,
                json!({
                    "error": "invalid",
                    "fields": {
                        "email": { "code": "invalid" },
                        "password": { "code": "too_short", "min": 12 },
                    },
                }),
            ),
            (
                ApiError::Unauthorized {
                    code: "sign_in_failed",
                },
                StatusCode::UNAUTHORIZED,
                json!({ "error": "sign_in_failed" }),
            ),
            (
                ApiError::Forbidden {
                    code: "cross_site_request",
                },
                StatusCode::FORBIDDEN,
                json!({ "error": "cross_site_request" }),
            ),
            (
                ApiError::Conflict {
                    code: "terms_changed",
                },
                StatusCode::CONFLICT,
                json!({ "error": "terms_changed" }),
            ),
            (
                ApiError::Gone {
                    code: "link_expired",
                },
                StatusCode::GONE,
                json!({ "error": "link_expired" }),
            ),
            (
                ApiError::PreconditionRequired {
                    code: "challenge_required",
                    details,
                },
                StatusCode::PRECONDITION_REQUIRED,
                json!({ "error": "challenge_required", "challenge": { "difficulty": 18 } }),
            ),
        ] {
            assert_eq!(parts(error).await, (status, None, body));
        }
    }

    #[tokio::test]
    async fn too_many_requests_says_when_to_retry_in_whole_seconds() {
        for (retry_after, header) in [
            (Duration::from_secs(20), "20"),
            (Duration::from_millis(20_001), "21"),
            (Duration::ZERO, "1"),
        ] {
            let (status, header_value, body) =
                parts(ApiError::TooManyRequests { retry_after }).await;

            assert_eq!(status, StatusCode::TOO_MANY_REQUESTS);
            assert_eq!(header_value.as_deref(), Some(header));
            assert_eq!(body, json!({ "error": "too_many_requests" }));
        }
    }

    #[tokio::test]
    async fn internal_errors_are_not_sent() {
        let err = std::io::Error::other("connection to 10.0.0.5 refused");

        let (status, _, body) = parts(ApiError::internal(err)).await;

        assert_eq!(status, StatusCode::INTERNAL_SERVER_ERROR);
        assert_eq!(body, json!({ "error": "internal" }));
    }
}
