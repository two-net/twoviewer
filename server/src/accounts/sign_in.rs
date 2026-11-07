//! `POST /api/sign-in`: signing in with an email address and a password (AUTH-05). Failed attempts
//! are throttled per address (AUTH-08) and per network (SEC-08), and no answer reveals whether an
//! address is registered (SEC-14).
//!
//! With MFA on, a right password doesn't sign in yet: it starts a [pending
//! sign-in](super::pending_sign_in), which the second step completes with a code
//! ([`super::sign_in_second_factor`]).

use std::{sync::Arc, time::Instant};

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::{StatusCode, header},
    response::{IntoResponse, Response},
};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, IntoActiveModel, QueryFilter, Set,
    TransactionTrait,
    prelude::{IpNetwork, Uuid},
};
use serde::Deserialize;
use serde_json::{Value, json};

use super::{
    Accounts, RESET_PASSWORD_PAGE, mfa,
    password::{Password, PasswordHash, Verification},
    pending_sign_in, sign_in_session, sign_up, signed_in,
    token::Token,
    user,
};
use crate::{
    api::{ApiError, FieldError, FieldErrors},
    audit::{self, Action, Actor, Event, Origin, Outcome, Target},
    db::EmailAddress,
    email::SignInBlocked,
    http::{ClientOrigin, CorrelationId},
    throttle::{self, Failure, Scope, Status},
};

#[derive(Deserialize)]
pub(super) struct Request {
    email: String,
    password: Password,
}

/// The answer to a wrong password, and alike to an address without an account (SEC-14).
const FAILED: ApiError = ApiError::Unauthorized {
    code: "sign_in_failed",
};

/// The code of the answer to a right password when MFA is on: the second step comes next.
const SECOND_FACTOR_REQUIRED: &str = "second_factor_required";

/// The reason codes of failed sign-ins in the audit trail.
mod reason {
    /// The address has no account, or the password is wrong. The event's target says which.
    pub const INVALID_CREDENTIALS: &str = "invalid_credentials";
    /// Sign-in with the address is blocked after too many failures (AUTH-08).
    pub const BLOCKED: &str = "blocked";
    /// The client's network has failed to sign in too often (SEC-08).
    pub const NETWORK_BLOCKED: &str = "network_blocked";
    /// The failures that started a block (`sign_in_blocked`).
    pub const TOO_MANY_FAILURES: &str = "too_many_failures";
}

/// Signs in, and answers `204 No Content` with the cookie of a new sign-in ([`signed_in`]).
///
/// Otherwise it answers `401` with `sign_in_failed`, the same whether the password is wrong or the
/// address has no account. The password is hashed either way, so the answer takes as long too.
/// After `sign_in.max_failures` failures with an address within `sign_in.failure_window`, sign-in
/// with it answers `429` for `sign_in.block_duration`, also with the right password, and also when
/// the address has no account, so that a block doesn't reveal it either. The owner of an account is
/// emailed when a block starts. A network that fails too often gets `429` as well.
///
/// Every attempt records one audit event: `sign_in_succeeded` or `sign_in_failed`. The failure
/// that starts a block also records `sign_in_blocked`.
///
/// With MFA on, a right password answers `428` (`second_factor_required`) instead, with the cookie
/// of a pending sign-in, and records nothing yet: the second step records the outcome.
pub(super) async fn sign_in(
    State(accounts): State<Arc<Accounts>>,
    CorrelationId(correlation_id): CorrelationId,
    ClientOrigin(origin): ClientOrigin,
    request: Result<Json<Request>, JsonRejection>,
) -> Result<Response, ApiError> {
    let Json(request) = request?;
    let email = validate(&request)?;
    let attempt = Attempt {
        accounts: &accounts,
        subject: throttle::subject(email.as_str()),
        user: user::Entity::find()
            .filter(user::Column::Email.eq(email))
            .one(&accounts.db)
            .await?,
        origin,
        correlation_id,
    };

    // Refused before the password is checked, so that guessing while blocked costs no hashing.
    if let Some(retry_after) = accounts
        .limits
        .sign_in_refused(attempt.origin.ip_address, Instant::now())
    {
        return attempt.refused(reason::NETWORK_BLOCKED, retry_after).await;
    }
    if let Status::Blocked { retry_after } =
        throttle::check(&accounts.db, Scope::SignIn, &attempt.subject).await?
    {
        return attempt.refused(reason::BLOCKED, retry_after).await;
    }

    // Also without an account, so that the answer takes as long (SEC-14).
    let stored = attempt.user.as_ref().map(|user| &user.password_hash);
    let verification = accounts
        .passwords
        .verify(&request.password, stored)
        .await
        .map_err(ApiError::internal)?;
    match (verification, attempt.user.clone()) {
        (Verification::Match { rehashed }, Some(user)) => attempt.succeeded(user, rehashed).await,
        _ => attempt.failed().await,
    }
}

/// Checks that both fields are there, and that the address could be one. Neither depends on
/// whether it is registered.
fn validate(request: &Request) -> Result<EmailAddress, ApiError> {
    let mut fields = FieldErrors::new();
    let email = sign_up::parse_email(&request.email)
        .inspect_err(|error| {
            fields.insert("email", *error);
        })
        .ok();
    if request.password.char_count() == 0 {
        fields.insert("password", FieldError::new("required"));
    }
    match email {
        Some(email) if fields.is_empty() => Ok(email),
        _ => Err(ApiError::Invalid(fields)),
    }
}

/// One sign-in attempt.
struct Attempt<'a> {
    accounts: &'a Accounts,
    /// The address as the throttle stores it.
    subject: Vec<u8>,
    /// The account with the address, if there is one.
    user: Option<user::Model>,
    origin: Origin,
    correlation_id: Uuid,
}

impl Attempt<'_> {
    /// Records the event of an attempt that didn't sign in. Its actor is a visitor: whoever tried
    /// isn't known to be the account's owner.
    async fn record_failure<C: ConnectionTrait>(
        &self,
        db: &C,
        action: Action,
        reason_code: &'static str,
        details: Value,
    ) -> Result<(), audit::Error> {
        audit::record(
            db,
            Event {
                actor: Actor::Visitor {
                    origin: self.origin.clone(),
                },
                action,
                target: self.user.as_ref().map(|user| Target::User(user.id)),
                outcome: Outcome::Failure { reason_code },
                correlation_id: self.correlation_id,
                details,
            },
        )
        .await?;
        Ok(())
    }

    /// Refuses an attempt without checking its password, with `429` until `retry_after`.
    async fn refused(
        &self,
        reason_code: &'static str,
        retry_after: std::time::Duration,
    ) -> Result<Response, ApiError> {
        // Before the response, though nothing changes (AUD-03).
        self.record_failure(
            &self.accounts.db,
            Action::SignInFailed,
            reason_code,
            json!({}),
        )
        .await?;
        self.accounts.metrics.sign_ins_failed.inc();
        Err(ApiError::TooManyRequests { retry_after })
    }

    /// Counts a wrong password, or an address without an account, the same way.
    async fn failed(&self) -> Result<Response, ApiError> {
        let accounts = self.accounts;
        let txn = accounts.db.begin().await?;
        let block_started = count_wrong_password(
            &txn,
            accounts,
            &self.subject,
            Event {
                actor: Actor::Visitor {
                    origin: self.origin.clone(),
                },
                action: Action::SignInFailed,
                target: self.user.as_ref().map(|user| Target::User(user.id)),
                outcome: Outcome::Failure {
                    reason_code: reason::INVALID_CREDENTIALS,
                },
                correlation_id: self.correlation_id,
                details: json!({}),
            },
        )
        .await?;
        txn.commit().await?;

        accounts.metrics.sign_ins_failed.inc();
        accounts
            .limits
            .sign_in_failed(self.origin.ip_address, Instant::now());
        if block_started && let Some(user) = &self.user {
            notify_blocked(accounts, user);
        }
        Err(FAILED)
    }

    /// Signs `user` in, whose password matched, unless a concurrent failure has blocked sign-in
    /// with the address in the meantime.
    async fn succeeded(
        self,
        user: user::Model,
        rehashed: Option<PasswordHash>,
    ) -> Result<Response, ApiError> {
        let accounts = self.accounts;
        let txn = accounts.db.begin().await?;
        if let Status::Blocked { retry_after } =
            throttle::clear_unless_blocked(&txn, Scope::SignIn, &self.subject).await?
        {
            txn.rollback().await?;
            return self.refused(reason::BLOCKED, retry_after).await;
        }
        let user_id = user.id;
        if let Some(hash) = rehashed {
            let mut user = user.into_active_model();
            user.password_hash = Set(hash);
            user.update(&txn).await?;
        }

        // With MFA on, the second step (AUTH-05) comes before the sign-in exists. Only now, once
        // the password was right, so that the answer reveals nothing to anyone else (SEC-14).
        if mfa::second_factor::enabled(&txn, user_id).await?.is_some() {
            let timeout = accounts.mfa.sign_in_step_timeout;
            let token = pending_sign_in::create(&txn, user_id, timeout).await?;
            txn.commit().await?;
            let mut response = ApiError::PreconditionRequired {
                code: SECOND_FACTOR_REQUIRED,
                details: Default::default(),
            }
            .into_response();
            response
                .headers_mut()
                .append(header::SET_COOKIE, pending_sign_in::cookie(&token, timeout));
            return Ok(response);
        }

        let token = start(
            &txn,
            accounts,
            user_id,
            self.origin,
            self.correlation_id,
            json!({}),
        )
        .await?;
        txn.commit().await?;
        Ok(signed_in_response(accounts, &token, false))
    }
}

/// Counts a wrong password for the email address `subject` (AUTH-08), and records `failure`, its
/// audit event, in the transaction `db`. Sign-in and re-authentication both count here, so that
/// guessing a password is throttled wherever it is typed.
///
/// When the failure starts a block, it also records `sign_in_blocked` by the same actor, and
/// returns `true`: once the transaction has committed, [`notify_blocked`] the account's owner.
pub(super) async fn count_wrong_password<C: ConnectionTrait>(
    db: &C,
    accounts: &Accounts,
    subject: &[u8],
    failure: Event,
) -> Result<bool, ApiError> {
    let counted =
        throttle::record_failure(db, Scope::SignIn, subject, &accounts.sign_in_rule()).await?;
    let blocked = Event {
        action: Action::SignInBlocked,
        outcome: Outcome::Failure {
            reason_code: reason::TOO_MANY_FAILURES,
        },
        details: json!({ "blocked_seconds": accounts.sign_in.block_duration.as_secs() }),
        ..failure.clone()
    };
    audit::record(db, failure).await?;
    let block_started = counted == Failure::BlockStarted;
    if block_started {
        audit::record(db, blocked).await?;
    }
    Ok(block_started)
}

/// Emails the owner of `user` that sign-in with their address is blocked, with a link to reset
/// their password.
pub(super) fn notify_blocked(accounts: &Accounts, user: &user::Model) {
    let reset_url = accounts.public_url.join(RESET_PASSWORD_PAGE);
    accounts.mailer.send(
        &user.email,
        &SignInBlocked {
            failures: accounts.sign_in.max_failures,
            blocked_for: accounts.sign_in.block_duration,
            reset_url: &reset_url,
        },
    );
}

/// Signs `user_id` in from `origin`: stores a new sign-in, which authenticated now, records
/// `sign_in_succeeded` with `details` and the sign-in's ID, and returns its token. Call it in the
/// transaction of the sign-in.
pub(super) async fn start<C: ConnectionTrait>(
    db: &C,
    accounts: &Accounts,
    user_id: Uuid,
    origin: Origin,
    correlation_id: Uuid,
    mut details: Value,
) -> Result<Token, ApiError> {
    let token = Token::generate();
    let now = Utc::now();
    let session = sign_in_session::ActiveModel {
        user_id: Set(user_id),
        token_hash: Set(token.hash()),
        created_at: Set(now),
        last_active_at: Set(now),
        expires_at: Set(accounts.lifetime.expiry(now, now)),
        authenticated_at: Set(now),
        ip_address: Set(IpNetwork::from(origin.ip_address)),
        client_description: Set(origin.client_description.clone()),
        revoked_at: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await?;
    details["sign_in_session_id"] = json!(session.id);
    audit::record(
        db,
        Event {
            actor: Actor::User {
                id: user_id,
                origin,
            },
            action: Action::SignInSucceeded,
            target: Some(Target::User(user_id)),
            outcome: Outcome::Success,
            correlation_id,
            details,
        },
    )
    .await?;
    // So that the user's open lists of sign-ins show it.
    sign_in_session::changed(db, user_id).await?;
    Ok(token)
}

/// `204` with the cookie of the sign-in whose token is `token`, and, after the second step, a
/// cookie that removes the pending sign-in's.
pub(super) fn signed_in_response(
    accounts: &Accounts,
    token: &Token,
    clear_pending: bool,
) -> Response {
    // Kept as long as the sign-in can last; the server ends it earlier if it isn't used.
    let session = signed_in::cookie(token, accounts.lifetime.max);
    let mut response = (StatusCode::NO_CONTENT, [(header::SET_COOKIE, session)]).into_response();
    if clear_pending {
        response
            .headers_mut()
            .append(header::SET_COOKIE, pending_sign_in::clear_cookie());
    }
    response
}
