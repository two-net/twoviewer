//! `POST /api/account/password`: changing the password of the signed-in user (AUTH-11), with the
//! current one. It needs a recent authentication too (AUTH-10), which with MFA on includes the
//! second factor.
//!
//! A wrong current password counts toward the same block as failed sign-ins with the user's
//! address (AUTH-08), as at re-authentication, so that a stolen sign-in cookie doesn't give more
//! guesses.

use std::sync::Arc;

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
};
use sea_orm::{
    ActiveModelTrait, EntityTrait, IntoActiveModel, QuerySelect, Set, TransactionTrait,
    prelude::Uuid,
};
use serde::Deserialize;
use serde_json::json;

use super::{
    Accounts, RecentlyAuthenticated,
    password::{NewPasswordError, Password, Verification},
    pending_sign_in, sign_in,
    sign_in_session::{self, Revoke},
    sign_up,
    signed_in::NOT_SIGNED_IN,
    user,
};
use crate::{
    api::{ApiError, FieldError, FieldErrors},
    audit::{self, Action, Actor, Event, Origin, Outcome, Target},
    http::{ClientOrigin, CorrelationId},
    throttle::{self, Scope, Status},
};

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Request {
    current_password: Password,
    new_password: Password,
}

/// The reason codes of failed password changes in the audit trail.
mod reason {
    /// The current password is wrong.
    pub const INVALID_CREDENTIALS: &str = "invalid_credentials";
    /// Passwords for the account are blocked after too many wrong ones.
    pub const BLOCKED: &str = "blocked";
}

/// Sets the signed-in user's password to `newPassword`, if `currentPassword` is right, and answers
/// `204 No Content`. Otherwise `422`: `currentPassword` is `required` or `incorrect`, or
/// `newPassword` breaks the [rules](super::password) for new passwords; both are reported at once,
/// as far as they can be before the current password is checked. `429` while passwords for the
/// account are blocked.
///
/// In one transaction, it stores the new password's hash, and signs out everywhere else: it
/// revokes every other sign-in of the account, and deletes its pending sign-ins, which were
/// started with the old password. The sign-in of the request stays.
///
/// It records `password_changed`, with its outcome, and `sign_in_session_revoked` for each sign-in
/// that ends. A wrong current password that starts a block also records `sign_in_blocked` and
/// emails the owner, as at sign-in.
pub(super) async fn change_password(
    State(accounts): State<Arc<Accounts>>,
    CorrelationId(correlation_id): CorrelationId,
    ClientOrigin(origin): ClientOrigin,
    RecentlyAuthenticated(signed_in): RecentlyAuthenticated,
    request: Result<Json<Request>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(request) = request?;
    let mut fields = FieldErrors::new();
    if request.current_password.char_count() == 0 {
        fields.insert("currentPassword", FieldError::new("required"));
    }
    if request.new_password.char_count() == 0 {
        fields.insert("newPassword", FieldError::new("required"));
    } else if let Err(rejected) = accounts.passwords.check_new(&request.new_password) {
        fields.extend(sign_up::password_error("newPassword", rejected));
    }
    if !fields.is_empty() {
        return Err(ApiError::Invalid(fields));
    }

    let user = user::Entity::find_by_id(signed_in.user_id)
        .one(&accounts.db)
        .await?
        .ok_or(NOT_SIGNED_IN)?;
    let attempt = Attempt {
        user_id: user.id,
        origin,
        correlation_id,
    };
    let subject = throttle::subject(user.email.as_str());
    if let Status::Blocked { retry_after } =
        throttle::check(&accounts.db, Scope::SignIn, &subject).await?
    {
        // Before the response, though nothing changes (AUD-03).
        audit::record(&accounts.db, attempt.failure(reason::BLOCKED)).await?;
        return Err(ApiError::TooManyRequests { retry_after });
    }

    let verification = accounts
        .passwords
        .verify(&request.current_password, Some(&user.password_hash))
        .await
        .map_err(ApiError::internal)?;
    if matches!(verification, Verification::Mismatch) {
        let txn = accounts.db.begin().await?;
        let block_started = sign_in::count_wrong_password(
            &txn,
            &accounts,
            &subject,
            attempt.failure(reason::INVALID_CREDENTIALS),
        )
        .await?;
        txn.commit().await?;
        if block_started {
            sign_in::notify_blocked(&accounts, &user);
        }
        return Err(ApiError::Invalid(FieldErrors::from([(
            "currentPassword",
            FieldError::new("incorrect"),
        )])));
    }
    // Before the transaction, so that no row stays locked while it is hashed.
    let password_hash = match accounts.passwords.hash_new(&request.new_password).await {
        Ok(hash) => hash,
        // `check_new` has already checked the rules.
        Err(NewPasswordError::Rejected(rejected)) => {
            return Err(ApiError::Invalid(sign_up::password_error(
                "newPassword",
                rejected,
            )));
        }
        Err(NewPasswordError::Hashing(err)) => return Err(ApiError::internal(err)),
    };

    let txn = accounts.db.begin().await?;
    // The password was right, so earlier wrong ones no longer count, unless a concurrent failure
    // has started a block meanwhile.
    if let Status::Blocked { retry_after } =
        throttle::clear_unless_blocked(&txn, Scope::SignIn, &subject).await?
    {
        txn.rollback().await?;
        audit::record(&accounts.db, attempt.failure(reason::BLOCKED)).await?;
        return Err(ApiError::TooManyRequests { retry_after });
    }
    let user = user::Entity::find_by_id(user.id)
        .lock_exclusive()
        .one(&txn)
        .await?
        .ok_or(NOT_SIGNED_IN)?;
    let mut user = user.into_active_model();
    user.password_hash = Set(password_hash);
    user.update(&txn).await?;
    audit::record(
        &txn,
        Event {
            outcome: Outcome::Success,
            ..attempt.failure(reason::INVALID_CREDENTIALS)
        },
    )
    .await?;
    sign_in_session::revoke(
        &txn,
        attempt.user_id,
        Revoke::AllExcept(signed_in.session_id),
        Action::SignInSessionRevoked,
        attempt.actor(),
        correlation_id,
    )
    .await?;
    pending_sign_in::remove_all(&txn, attempt.user_id).await?;
    // #32: email the user that the password was changed (AUTH-12).
    txn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// One attempt to change the password.
struct Attempt {
    user_id: Uuid,
    origin: Origin,
    correlation_id: Uuid,
}

impl Attempt {
    fn actor(&self) -> Actor {
        Actor::User {
            id: self.user_id,
            origin: self.origin.clone(),
        }
    }

    /// The event of an attempt that failed for `reason_code`.
    fn failure(&self, reason_code: &'static str) -> Event {
        Event {
            actor: self.actor(),
            action: Action::PasswordChanged,
            target: Some(Target::User(self.user_id)),
            outcome: Outcome::Failure { reason_code },
            correlation_id: self.correlation_id,
            details: json!({}),
        }
    }
}
