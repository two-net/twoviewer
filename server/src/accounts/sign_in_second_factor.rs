//! `POST /api/sign-in/second-factor`: the second step of signing in with MFA on (AUTH-05): a code
//! from the authenticator app, or an unused recovery code.
//!
//! The browser holds the token of its [pending sign-in](super::pending_sign_in) in a cookie,
//! which the first step set once the password was right. A right code turns it into a sign-in.
//! Wrong codes are throttled per user ([`super::mfa::second_factor`], SEC-08), and count toward
//! the limit per network like wrong passwords do.

use std::{sync::Arc, time::Instant};

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::{HeaderMap, header},
    response::{IntoResponse, Response},
};
use sea_orm::{TransactionTrait, prelude::Uuid};
use serde::Deserialize;
use serde_json::json;

use super::{
    Accounts,
    mfa::second_factor::{self, TypedCode},
    pending_sign_in, sign_in,
};
use crate::{
    api::{ApiError, FieldError, FieldErrors},
    audit::{self, Action, Actor, Event, Origin, Outcome, Target},
    http::{ClientOrigin, CorrelationId},
    throttle::Status,
};

#[derive(Deserialize)]
pub(super) struct Request {
    code: TypedCode,
}

/// The answer to a wrong code.
const FAILED: ApiError = ApiError::Unauthorized {
    code: "second_factor_failed",
};

/// The code of the answer to a second step without a pending sign-in: it has expired, has been
/// completed, or never was.
const STEP_EXPIRED: &str = "sign_in_step_expired";

/// The reason codes of failed second steps in the audit trail.
mod reason {
    /// The code is wrong, or has been used.
    pub const INVALID_SECOND_FACTOR: &str = "invalid_second_factor";
    /// Codes for the account are blocked after too many wrong ones (SEC-08).
    pub const BLOCKED: &str = "blocked";
    /// The client's network has failed to sign in too often (SEC-08).
    pub const NETWORK_BLOCKED: &str = "network_blocked";
}

/// Completes the pending sign-in in the request's cookie with a right code, and answers `204` with
/// the cookie of the new sign-in, and one that removes the pending sign-in's.
///
/// Otherwise it answers `401` (`second_factor_failed`) for a wrong code, `410`
/// (`sign_in_step_expired`) without a pending sign-in, which means signing in again, `422` for
/// an empty code, or `429` while the user's codes or the client's network are blocked.
///
/// A right code records `sign_in_succeeded` with the factor used, and a recovery code also
/// `recovery_code_used`. A wrong one records `sign_in_failed`, and `sign_in_blocked` if it starts a
/// block.
pub(super) async fn second_factor(
    State(accounts): State<Arc<Accounts>>,
    CorrelationId(correlation_id): CorrelationId,
    ClientOrigin(origin): ClientOrigin,
    headers: HeaderMap,
    request: Result<Json<Request>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(request) = request?;
    if request.code.is_blank() {
        let mut fields = FieldErrors::new();
        fields.insert("code", FieldError::new("required"));
        return Err(ApiError::Invalid(fields));
    }
    let Some(pending) = pending_sign_in::find(&accounts.db, &headers).await? else {
        return Ok(step_expired());
    };
    // MFA was turned off since the password step: sign in again, without one.
    let Some(factor) = second_factor::enabled(&accounts.db, pending.user_id).await? else {
        return Ok(step_expired());
    };
    let step = Step {
        accounts: &accounts,
        user_id: pending.user_id,
        origin,
        correlation_id,
    };

    if let Some(retry_after) = accounts
        .limits
        .sign_in_refused(step.origin.ip_address, Instant::now())
    {
        return step.refused(reason::NETWORK_BLOCKED, retry_after).await;
    }
    if let Some(retry_after) = second_factor::blocked(&accounts.db, step.user_id).await? {
        return step.refused(reason::BLOCKED, retry_after).await;
    }

    let txn = accounts.db.begin().await?;
    let verified = second_factor::verify(
        &txn,
        &accounts.mfa_cipher,
        &accounts.mfa,
        &factor,
        &request.code,
    )
    .await?;
    let Some(method) = verified else {
        second_factor::failed(
            &txn,
            &accounts.mfa,
            step.user_id,
            step.failure(reason::INVALID_SECOND_FACTOR),
        )
        .await?;
        txn.commit().await?;
        accounts.metrics.sign_ins_failed.inc();
        accounts
            .limits
            .sign_in_failed(step.origin.ip_address, Instant::now());
        return Err(FAILED);
    };

    let actor = Actor::User {
        id: step.user_id,
        origin: step.origin.clone(),
    };
    if let Status::Blocked { retry_after } =
        second_factor::succeeded(&txn, step.user_id, method, actor, correlation_id).await?
    {
        // So that the code isn't used up.
        txn.rollback().await?;
        return step.refused(reason::BLOCKED, retry_after).await;
    }
    if !pending_sign_in::complete(&txn, pending.id).await? {
        txn.rollback().await?;
        return Ok(step_expired());
    }
    let token = sign_in::start(
        &txn,
        &accounts,
        step.user_id,
        step.origin,
        correlation_id,
        json!({ "second_factor": method.name() }),
    )
    .await?;
    txn.commit().await?;
    Ok(sign_in::signed_in_response(&accounts, &token, true))
}

/// `410` (`sign_in_step_expired`), with a cookie that removes the pending sign-in's.
fn step_expired() -> Response {
    let mut response = ApiError::Gone { code: STEP_EXPIRED }.into_response();
    response
        .headers_mut()
        .append(header::SET_COOKIE, pending_sign_in::clear_cookie());
    response
}

/// One attempt at the second step.
struct Step<'a> {
    accounts: &'a Accounts,
    user_id: Uuid,
    origin: Origin,
    correlation_id: Uuid,
}

impl Step<'_> {
    /// The event of an attempt that didn't sign in. Its actor is a visitor, as at the first step:
    /// whoever tried isn't known to be the account's owner.
    fn failure(&self, reason_code: &'static str) -> Event {
        Event {
            actor: Actor::Visitor {
                origin: self.origin.clone(),
            },
            action: Action::SignInFailed,
            target: Some(Target::User(self.user_id)),
            outcome: Outcome::Failure { reason_code },
            correlation_id: self.correlation_id,
            details: json!({}),
        }
    }

    /// Refuses an attempt without checking its code, with `429` until `retry_after`.
    async fn refused(
        &self,
        reason_code: &'static str,
        retry_after: std::time::Duration,
    ) -> Result<Response, ApiError> {
        // Before the response, though nothing changes (AUD-03).
        audit::record(&self.accounts.db, self.failure(reason_code)).await?;
        self.accounts.metrics.sign_ins_failed.inc();
        Err(ApiError::TooManyRequests { retry_after })
    }
}
