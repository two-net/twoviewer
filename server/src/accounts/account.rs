//! The signed-in user's account:
//!
//! - `GET /api/account` answers it ([`account`]).
//! - `PATCH /api/account` changes the display name ([`update`], AUTH-11).
//!
//! Changing the email address and the password have endpoints of their own
//! ([`super::change_email`], [`super::change_password`]), since both need a recent authentication
//! (AUTH-10).

use std::sync::Arc;

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, IntoActiveModel, QueryFilter,
    QueryOrder, QuerySelect, Set, TransactionTrait, prelude::Uuid,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use twoviewer_protocol::Event as RealtimeEvent;

use super::{
    Accounts,
    email_link::{self, Purpose},
    sign_up,
    signed_in::{NOT_SIGNED_IN, SignedIn},
    user,
};
use crate::{
    api::{ApiError, FieldErrors},
    audit::{self, Action, Actor, Event, Outcome, Target},
    db::EmailAddress,
    http::{ClientOrigin, CorrelationId},
    realtime::{self, Topic},
};

/// The event that tells a user's browsers that their account has changed: its display name or
/// email address, or a change of address waiting to be confirmed. Open settings then reload it.
pub const CHANGED_EVENT: &str = "account.changed";

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Account {
    id: Uuid,
    email: String,
    display_name: String,
    /// Until it is, the user cannot pair devices or start sessions (AUTH-02).
    email_verified: bool,
    /// The address the user asked to change theirs to, while the link that confirms it works.
    #[serde(skip_serializing_if = "Option::is_none")]
    pending_email: Option<String>,
}

/// Answers the account of the signed-in user, or `401` without a sign-in.
pub(super) async fn account(
    State(accounts): State<Arc<Accounts>>,
    signed_in: SignedIn,
) -> Result<Json<Account>, ApiError> {
    // Deleting an account deletes its sign-ins, so this finds one but for a concurrent deletion.
    let user = user::Entity::find_by_id(signed_in.user_id)
        .one(&accounts.db)
        .await?
        .ok_or(NOT_SIGNED_IN)?;
    let pending_email = pending_email(&accounts.db, user.id).await?;
    Ok(Json(Account {
        id: user.id,
        email_verified: user.is_verified(),
        email: user.email.as_str().to_owned(),
        display_name: user.display_name,
        pending_email: pending_email.map(|email| email.as_str().to_owned()),
    }))
}

/// The new address of `user_id`'s newest link that changes their address, if it still works.
/// Asking for another change uses up the earlier links, so there is at most one.
async fn pending_email<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
) -> Result<Option<EmailAddress>, sea_orm::DbErr> {
    let link = email_link::Entity::find()
        .filter(email_link::Column::UserId.eq(user_id))
        .filter(email_link::Column::Purpose.eq(Purpose::ChangeEmail))
        .filter(email_link::Column::UsedAt.is_null())
        .filter(email_link::Column::ExpiresAt.gt(Utc::now()))
        .order_by_desc(email_link::Column::CreatedAt)
        .limit(1)
        .one(db)
        .await?;
    Ok(link.and_then(|link| link.new_email))
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct UpdateRequest {
    display_name: String,
}

/// Changes the display name of the signed-in user, and answers `204 No Content`, or `422` for a
/// name that breaks the rules of sign-up. A name that is the same as before changes nothing and
/// records nothing; any other records `display_name_changed`. It needs no recent authentication:
/// the name is shown to other people, but doesn't let anyone in.
pub(super) async fn update(
    State(accounts): State<Arc<Accounts>>,
    CorrelationId(correlation_id): CorrelationId,
    ClientOrigin(origin): ClientOrigin,
    signed_in: SignedIn,
    request: Result<Json<UpdateRequest>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(request) = request?;
    let display_name = sign_up::parse_display_name(
        &request.display_name,
        accounts.config.display_name_max_length,
    )
    .map_err(|error| ApiError::Invalid(FieldErrors::from([("displayName", error)])))?;

    let txn = accounts.db.begin().await?;
    let user = user::Entity::find_by_id(signed_in.user_id)
        .lock_exclusive()
        .one(&txn)
        .await?
        .ok_or(NOT_SIGNED_IN)?;
    if user.display_name == display_name {
        return Ok(StatusCode::NO_CONTENT);
    }
    let user_id = user.id;
    let mut user = user.into_active_model();
    user.display_name = Set(display_name);
    user.update(&txn).await?;
    audit::record(
        &txn,
        Event {
            actor: Actor::User {
                id: user_id,
                origin,
            },
            action: Action::DisplayNameChanged,
            target: Some(Target::User(user_id)),
            outcome: Outcome::Success,
            correlation_id,
            // Not the names: they are personal data, which the account keeps.
            details: json!({}),
        },
    )
    .await?;
    changed(&txn, user_id).await?;
    txn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// Tells `user_id`'s browsers that their account has changed, once the transaction of `db`
/// commits.
pub(super) async fn changed<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
) -> Result<(), realtime::PublishError> {
    realtime::publish(
        db,
        Topic::User(user_id),
        &RealtimeEvent {
            name: CHANGED_EVENT.to_owned(),
            data: json!({}),
        },
    )
    .await
}
