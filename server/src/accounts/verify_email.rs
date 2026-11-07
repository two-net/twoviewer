//! `POST /api/verify-email`: verifying an email address with the link from the email (AUTH-02).

use std::sync::Arc;

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, QuerySelect, Set,
    TransactionTrait,
};
use serde::Deserialize;
use serde_json::json;

use super::{
    Accounts,
    email_link::{self, MAX_TOKEN_LENGTH, Purpose},
    user,
};
use crate::{
    api::ApiError,
    audit::{self, Action, Actor, Event, Outcome, Target},
    http::{ClientOrigin, CorrelationId},
};

#[derive(Deserialize)]
pub(super) struct Request {
    /// The token from the link. It is a secret: never log it.
    token: String,
}

const EXPIRED: ApiError = ApiError::Gone {
    code: "link_expired",
};

/// Verifies the address of the link's account, and answers `204 No Content`.
///
/// A link works once, until it expires. Opening it again answers `204` too while the address is
/// verified, without verifying it again: email scanners often open links before people do, and
/// the person should then still read that their address is verified. Any other link answers
/// `410 Gone`, whether it expired, was never issued, or its account is gone.
pub(super) async fn verify_email(
    State(accounts): State<Arc<Accounts>>,
    CorrelationId(correlation_id): CorrelationId,
    ClientOrigin(origin): ClientOrigin,
    request: Result<Json<Request>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(request) = request?;
    if request.token.len() > MAX_TOKEN_LENGTH {
        return Err(EXPIRED);
    }

    let txn = accounts.db.begin().await?;
    // Locked, so that two requests with the same link cannot both use it.
    let Some(link) = email_link::Entity::find()
        .filter(email_link::Column::TokenHash.eq(email_link::hash(&request.token)))
        .filter(email_link::Column::Purpose.eq(Purpose::VerifyEmail))
        .lock_exclusive()
        .one(&txn)
        .await?
    else {
        return Err(EXPIRED);
    };
    let Some(user) = user::Entity::find_by_id(link.user_id)
        .lock_exclusive()
        .one(&txn)
        .await?
    else {
        return Err(EXPIRED);
    };
    if user.is_verified() {
        return Ok(StatusCode::NO_CONTENT);
    }
    let now = Utc::now();
    if link.used_at.is_some() || link.expires_at <= now {
        return Err(EXPIRED);
    }

    let user_id = user.id;
    let mut link = link.into_active_model();
    link.used_at = Set(Some(now));
    link.update(&txn).await?;
    let mut user = user.into_active_model();
    user.email_verified_at = Set(Some(now));
    user.update(&txn).await?;
    audit::record(
        &txn,
        Event {
            actor: Actor::User {
                id: user_id,
                origin,
            },
            action: Action::EmailVerified,
            target: Some(Target::User(user_id)),
            outcome: Outcome::Success,
            correlation_id,
            details: json!({}),
        },
    )
    .await?;
    txn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}
