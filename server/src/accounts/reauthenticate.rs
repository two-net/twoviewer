//! `POST /api/reauthenticate`: entering the password again, and a second factor if MFA is on,
//! before a sensitive change (AUTH-10). It makes the sign-in of the request count as recently
//! authenticated ([`super::RecentlyAuthenticated`]) for `sign_in.reauthenticate_after`.
//!
//! Wrong passwords count toward the same block as failed sign-ins with the user's address
//! (AUTH-08), and wrong codes toward the same block as the second step of sign-in (SEC-08), so
//! that a stolen sign-in cookie doesn't give more guesses at either.

use std::sync::Arc;

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, Set,
    TransactionTrait, prelude::Uuid,
};
use serde::Deserialize;
use serde_json::json;

use super::{
    Accounts, SignedIn,
    mfa::second_factor::{self, TypedCode},
    password::{Password, Verification},
    sign_in, sign_in_session,
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
pub(super) struct Request {
    password: Password,
    /// From the authenticator app, or a recovery code. Required with MFA on, ignored without.
    #[serde(default)]
    code: TypedCode,
}

/// The answer to a wrong password or a wrong code alike, so that a stolen sign-in cookie doesn't
/// tell which of the two was right.
const FAILED: ApiError = ApiError::Unauthorized {
    code: "reauthentication_failed",
};

/// The reason codes of failed re-authentications in the audit trail.
mod reason {
    /// The password or the code is wrong. Only the counts of blocks tell which.
    pub const INVALID_CREDENTIALS: &str = "invalid_credentials";
    /// Passwords or codes for the account are blocked after too many wrong ones.
    pub const BLOCKED: &str = "blocked";
}

/// Checks the password of the signed-in user, and their second factor if MFA is on, and answers
/// `204` once their sign-in counts as recently authenticated. Otherwise `401`
/// (`reauthentication_failed`), `422` for a missing password or code, or `429` while passwords or
/// codes for the account are blocked.
///
/// Every attempt records `reauthenticated`, with its outcome. A wrong password that starts a block
/// also records `sign_in_blocked` and emails the owner, as at sign-in. A recovery code also
/// records `recovery_code_used`.
pub(super) async fn reauthenticate(
    State(accounts): State<Arc<Accounts>>,
    CorrelationId(correlation_id): CorrelationId,
    ClientOrigin(origin): ClientOrigin,
    signed_in: SignedIn,
    request: Result<Json<Request>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(request) = request?;
    let user = user::Entity::find_by_id(signed_in.user_id)
        .one(&accounts.db)
        .await?
        .ok_or(NOT_SIGNED_IN)?;
    let factor = second_factor::enabled(&accounts.db, user.id).await?;
    let mut fields = FieldErrors::new();
    if request.password.char_count() == 0 {
        fields.insert("password", FieldError::new("required"));
    }
    if factor.is_some() && request.code.is_blank() {
        fields.insert("code", FieldError::new("required"));
    }
    if !fields.is_empty() {
        return Err(ApiError::Invalid(fields));
    }

    let attempt = Attempt {
        accounts: &accounts,
        user_id: user.id,
        session_id: signed_in.session_id,
        origin,
        correlation_id,
    };
    let subject = throttle::subject(user.email.as_str());
    if let Status::Blocked { retry_after } =
        throttle::check(&accounts.db, Scope::SignIn, &subject).await?
    {
        return attempt.refused(retry_after).await;
    }
    if factor.is_some()
        && let Some(retry_after) = second_factor::blocked(&accounts.db, user.id).await?
    {
        return attempt.refused(retry_after).await;
    }

    let verification = accounts
        .passwords
        .verify(&request.password, Some(&user.password_hash))
        .await
        .map_err(ApiError::internal)?;
    let Verification::Match { rehashed } = verification else {
        let txn = accounts.db.begin().await?;
        let block_started =
            sign_in::count_wrong_password(&txn, &accounts, &subject, attempt.failure()).await?;
        txn.commit().await?;
        if block_started {
            sign_in::notify_blocked(&accounts, &user);
        }
        return Err(FAILED);
    };

    let txn = accounts.db.begin().await?;
    let method = match &factor {
        None => None,
        Some(factor) => {
            let verified = second_factor::verify(
                &txn,
                &accounts.mfa_cipher,
                &accounts.mfa,
                factor,
                &request.code,
            )
            .await?;
            if verified.is_none() {
                second_factor::failed(&txn, &accounts.mfa, user.id, attempt.failure()).await?;
                txn.commit().await?;
                return Err(FAILED);
            }
            verified
        }
    };
    // The password was right, so earlier wrong ones no longer count, unless a concurrent failure
    // has started a block meanwhile.
    if let Status::Blocked { retry_after } =
        throttle::clear_unless_blocked(&txn, Scope::SignIn, &subject).await?
    {
        txn.rollback().await?;
        return attempt.refused(retry_after).await;
    }
    if let Some(method) = method
        && let Status::Blocked { retry_after } =
            second_factor::succeeded(&txn, user.id, method, attempt.actor(), correlation_id).await?
    {
        txn.rollback().await?;
        return attempt.refused(retry_after).await;
    }
    if let Some(hash) = rehashed {
        let mut user = user.into_active_model();
        user.password_hash = Set(hash);
        user.update(&txn).await?;
    }
    // Not if it was revoked in the meantime.
    let updated = sign_in_session::Entity::update_many()
        .set(sign_in_session::ActiveModel {
            authenticated_at: Set(Utc::now()),
            ..Default::default()
        })
        .filter(sign_in_session::Column::Id.eq(signed_in.session_id))
        .filter(sign_in_session::Column::RevokedAt.is_null())
        .exec(&txn)
        .await?;
    if updated.rows_affected == 0 {
        txn.rollback().await?;
        return Err(NOT_SIGNED_IN);
    }
    let mut details = json!({ "sign_in_session_id": signed_in.session_id });
    if let Some(method) = method {
        details["second_factor"] = json!(method.name());
    }
    audit::record(
        &txn,
        Event {
            outcome: Outcome::Success,
            details,
            ..attempt.failure()
        },
    )
    .await?;
    txn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// One attempt to re-authenticate.
struct Attempt<'a> {
    accounts: &'a Accounts,
    user_id: Uuid,
    session_id: Uuid,
    origin: Origin,
    correlation_id: Uuid,
}

impl Attempt<'_> {
    /// The user: unlike at sign-in, they are signed in.
    fn actor(&self) -> Actor {
        Actor::User {
            id: self.user_id,
            origin: self.origin.clone(),
        }
    }

    /// The event of a wrong password or code.
    fn failure(&self) -> Event {
        self.event(reason::INVALID_CREDENTIALS)
    }

    fn event(&self, reason_code: &'static str) -> Event {
        Event {
            actor: self.actor(),
            action: Action::Reauthenticated,
            target: Some(Target::User(self.user_id)),
            outcome: Outcome::Failure { reason_code },
            correlation_id: self.correlation_id,
            details: json!({ "sign_in_session_id": self.session_id }),
        }
    }

    /// Refuses an attempt without checking it, with `429` until `retry_after`.
    async fn refused(&self, retry_after: std::time::Duration) -> Result<StatusCode, ApiError> {
        // Before the response, though nothing changes (AUD-03).
        audit::record(&self.accounts.db, self.event(reason::BLOCKED)).await?;
        Err(ApiError::TooManyRequests { retry_after })
    }
}
