//! The signed-in user's sign-ins (AUTH-09): listing them, revoking them, and signing out.
//!
//! - `GET /api/sign-ins` lists the user's active sign-ins.
//! - `DELETE /api/sign-ins/{id}` revokes one of them.
//! - `DELETE /api/sign-ins` revokes all of them but the one of the request.
//! - `POST /api/sign-out` ends the sign-in of the request.
//!
//! Revoking records `sign_in_session_revoked` and signing out `sign_out`, one event per sign-in
//! that ends (Table 3.8-A). Either closes the browser's real-time connection, which takes it to
//! the sign-in page, and tells the user's other browsers to reload their list.

use std::sync::Arc;

use axum::{
    Json,
    extract::{FromRef, Path, State, rejection::PathRejection},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use sea_orm::{
    ColumnTrait, EntityTrait, QueryFilter, QueryOrder, TransactionTrait,
    prelude::{DateTimeUtc, Uuid},
};
use serde::Serialize;

use super::{
    Accounts, Authenticator, SignedIn,
    sign_in_session::{self, Revoke},
    signed_in,
};
use crate::{
    api::ApiError,
    audit::{Action, Actor},
    http::{ClientOrigin, CorrelationId},
};

/// The answer to revoking a sign-in that has already ended, or isn't the user's.
const SIGN_IN_ENDED: ApiError = ApiError::Gone {
    code: "sign_in_ended",
};

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct SignIns {
    sign_ins: Vec<SignIn>,
}

/// An active sign-in, as its user sees it.
#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct SignIn {
    id: Uuid,
    /// Browser and operating system, such as "Firefox on Linux".
    client: String,
    /// Where the user signed in from.
    ip_address: String,
    signed_in_at: DateTimeUtc,
    /// To the minute.
    last_active_at: DateTimeUtc,
    /// Whether it is the sign-in of the request.
    current: bool,
}

/// Answers the signed-in user's active sign-ins: the one of the request first, then the most
/// recently used first.
pub(super) async fn list(
    State(accounts): State<Arc<Accounts>>,
    signed_in: SignedIn,
) -> Result<Json<SignIns>, ApiError> {
    let mut sessions = sign_in_session::Entity::find()
        .filter(sign_in_session::Column::UserId.eq(signed_in.user_id))
        .filter(sign_in_session::Column::RevokedAt.is_null())
        .filter(sign_in_session::Column::ExpiresAt.gt(Utc::now()))
        .order_by_desc(sign_in_session::Column::LastActiveAt)
        .order_by_desc(sign_in_session::Column::CreatedAt)
        .all(&accounts.db)
        .await?;
    // Stable, so the others stay in order.
    sessions.sort_by_key(|session| session.id != signed_in.session_id);
    let sign_ins = sessions
        .into_iter()
        .map(|session| SignIn {
            current: session.id == signed_in.session_id,
            id: session.id,
            client: session.client_description,
            ip_address: session.ip_address.ip().to_string(),
            signed_in_at: session.created_at,
            last_active_at: session.last_active_at,
        })
        .collect();
    Ok(Json(SignIns { sign_ins }))
}

/// Revokes the user's sign-in `id`, and answers `204`. Revoking the sign-in of the request also
/// removes its cookie.
///
/// It answers `410` (`sign_in_ended`) if `id` isn't an active sign-in of the user: also if it is
/// another user's, so that nobody can tell (AUTH-17).
pub(super) async fn revoke(
    State(accounts): State<Arc<Accounts>>,
    CorrelationId(correlation_id): CorrelationId,
    ClientOrigin(origin): ClientOrigin,
    signed_in: SignedIn,
    id: Result<Path<Uuid>, PathRejection>,
) -> Result<Response, ApiError> {
    let Path(id) = id?;
    let txn = accounts.db.begin().await?;
    let revoked = sign_in_session::revoke(
        &txn,
        signed_in.user_id,
        Revoke::One(id),
        Action::SignInSessionRevoked,
        Actor::User {
            id: signed_in.user_id,
            origin,
        },
        correlation_id,
    )
    .await?;
    txn.commit().await?;

    if revoked.is_empty() {
        Err(SIGN_IN_ENDED)
    } else if id == signed_in.session_id {
        Ok(signed_out())
    } else {
        Ok(StatusCode::NO_CONTENT.into_response())
    }
}

/// Revokes all of the user's sign-ins but the one of the request, and answers `204`.
pub(super) async fn revoke_others(
    State(accounts): State<Arc<Accounts>>,
    CorrelationId(correlation_id): CorrelationId,
    ClientOrigin(origin): ClientOrigin,
    signed_in: SignedIn,
) -> Result<StatusCode, ApiError> {
    let txn = accounts.db.begin().await?;
    sign_in_session::revoke(
        &txn,
        signed_in.user_id,
        Revoke::AllExcept(signed_in.session_id),
        Action::SignInSessionRevoked,
        Actor::User {
            id: signed_in.user_id,
            origin,
        },
        correlation_id,
    )
    .await?;
    txn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Ends the sign-in of the request, and answers `204` with a cookie that removes it.
///
/// It answers the same without an active sign-in, so that a browser whose sign-in has already
/// ended also loses its cookie. Only ending an active one is audited.
pub(super) async fn sign_out(
    State(accounts): State<Arc<Accounts>>,
    CorrelationId(correlation_id): CorrelationId,
    ClientOrigin(origin): ClientOrigin,
    headers: HeaderMap,
) -> Result<Response, ApiError> {
    if let Some(signed_in) = Authenticator::from_ref(&accounts)
        .signed_in(&headers)
        .await?
    {
        let txn = accounts.db.begin().await?;
        sign_in_session::revoke(
            &txn,
            signed_in.user_id,
            Revoke::One(signed_in.session_id),
            Action::SignOut,
            Actor::User {
                id: signed_in.user_id,
                origin,
            },
            correlation_id,
        )
        .await?;
        txn.commit().await?;
    }
    Ok(signed_out())
}

/// `204`, with a cookie that removes the sign-in cookie.
fn signed_out() -> Response {
    (
        StatusCode::NO_CONTENT,
        [(header::SET_COOKIE, signed_in::clear_cookie())],
    )
        .into_response()
}
