//! Password reset by email (AUTH-07): a single-use link, valid for `password_reset.link_lifetime`
//! (1 hour), that sets a new password and revokes all of the account's sign-ins.
//!
//! - `POST /api/password-reset` ([`request`]) emails the link, and answers the same whether or not
//!   the address is registered (SEC-14).
//! - `POST /api/password-reset/check` ([`check`]) says whether a link still works, without using
//!   it, so the page can say so before the user chooses a password.
//! - `POST /api/password-reset/complete` ([`complete`]) sets the new password.
//!
//! Requests are limited per network and emails per address ([`super::limits`], SEC-08). Links
//! carry a 256-bit token, which cannot be guessed, so the other two need no limit of their own.

use std::{sync::Arc, time::Instant};

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, DbErr, EntityTrait, IntoActiveModel,
    QueryFilter, QuerySelect, Set, TransactionTrait,
};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};

use super::{
    Accounts, RESET_PASSWORD_PAGE,
    email_link::{self, MAX_TOKEN_LENGTH, Purpose, Token},
    password::{NewPasswordError, Password},
    pending_sign_in,
    sign_in_session::{self, Revoke},
    sign_up, user,
};
use crate::{
    api::{ApiError, FieldError, FieldErrors},
    audit::{self, Action, Actor, Event, Origin, Outcome, Target},
    db::EmailAddress,
    email::ResetPassword,
    http::{ClientOrigin, CorrelationId},
    throttle::{self, Scope},
};

/// The answer to a link that has been used or has expired, or never existed.
const EXPIRED: ApiError = ApiError::Gone {
    code: "link_expired",
};

/// The reason code of a request refused because its network asked too often (SEC-08).
const NETWORK_BLOCKED: &str = "network_blocked";

#[derive(Deserialize)]
pub(super) struct ResetRequest {
    email: String,
}

/// Emails a link that resets the password of the address's account, and answers `202 Accepted`
/// with no body, whether or not the address has one (SEC-14). Addresses without one get no email.
///
/// An address gets at most one email per `password_reset.email_interval`; requests in between get
/// the same answer, without one. Earlier links keep working until one of them is used. A network
/// that has asked `password_reset.max_per_network` times within `password_reset.window` gets `429`.
///
/// Every request records `password_reset_requested`, by a visitor, since whoever asked isn't known
/// to be the owner, with the account as the target if there is one, and whether it was `emailed`.
pub(super) async fn request(
    State(accounts): State<Arc<Accounts>>,
    CorrelationId(correlation_id): CorrelationId,
    ClientOrigin(origin): ClientOrigin,
    request: Result<Json<ResetRequest>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(request) = request?;
    let email = sign_up::parse_email(&request.email)
        .map_err(|error| ApiError::Invalid(FieldErrors::from([("email", error)])))?;
    let now = Instant::now();

    let txn = accounts.db.begin().await?;
    let user = user::Entity::find()
        .filter(user::Column::Email.eq(email))
        .one(&txn)
        .await?;
    let requested = Requested {
        origin: &origin,
        target: user.as_ref().map(|user| Target::User(user.id)),
        correlation_id,
    };
    if let Err(retry_after) = accounts.limits.admit_reset(origin.ip_address, now) {
        // Nothing changes, but the event is written before the answer (AUD-03).
        requested
            .record(
                &txn,
                Outcome::Failure {
                    reason_code: NETWORK_BLOCKED,
                },
                json!({}),
            )
            .await?;
        txn.commit().await?;
        return Err(ApiError::TooManyRequests { retry_after });
    }

    let link = match user {
        Some(user) if accounts.limits.may_email_reset(&user.email, now) => {
            let token = email_link::create(
                &txn,
                user.id,
                Purpose::ResetPassword,
                accounts.password_reset.link_lifetime,
            )
            .await?;
            Some((user.email, token))
        }
        _ => None,
    };
    requested
        .record(&txn, Outcome::Success, json!({ "emailed": link.is_some() }))
        .await?;
    txn.commit().await?;

    if let Some((to, token)) = link {
        accounts.send_reset(&to, &token);
    }
    Ok(StatusCode::ACCEPTED)
}

/// The event of one reset request.
struct Requested<'a> {
    origin: &'a Origin,
    target: Option<Target>,
    correlation_id: sea_orm::prelude::Uuid,
}

impl Requested<'_> {
    async fn record<C: ConnectionTrait>(
        &self,
        db: &C,
        outcome: Outcome,
        details: Value,
    ) -> Result<(), audit::Error> {
        audit::record(
            db,
            Event {
                actor: Actor::Visitor {
                    origin: self.origin.clone(),
                },
                action: Action::PasswordResetRequested,
                target: self.target,
                outcome,
                correlation_id: self.correlation_id,
                details,
            },
        )
        .await?;
        Ok(())
    }
}

#[derive(Deserialize)]
pub(super) struct CheckRequest {
    /// The token from the link. It is a secret: never log it.
    token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Link {
    /// The address of the link's account, which the page shows, and gives password managers to
    /// save the new password under. Whoever has the link has the email it came in.
    email: String,
}

/// Answers `200 OK` with the account's address if the link still works, or `410 Gone` if it has
/// been used or has expired, or never existed. It changes nothing, so email scanners that open
/// links before people do don't use it up.
pub(super) async fn check(
    State(accounts): State<Arc<Accounts>>,
    request: Result<Json<CheckRequest>, JsonRejection>,
) -> Result<Json<Link>, ApiError> {
    let Json(request) = request?;
    let (_, user) = find_link(&accounts.db, &request.token, false)
        .await?
        .ok_or(EXPIRED)?;
    Ok(Json(Link {
        email: user.email.as_str().to_owned(),
    }))
}

#[derive(Deserialize)]
pub(super) struct CompleteRequest {
    /// The token from the link. It is a secret: never log it.
    token: String,
    password: Password,
}

/// Sets the new password of the link's account, and answers `204 No Content`.
///
/// It answers `410 Gone` for a link that has been used or has expired, or never existed, before
/// looking at the password, so that only a working link costs a hash; and `422` for a password
/// that breaks the rules (AUTH-03, AUTH-04), which leaves the link working.
///
/// In one transaction, it:
///
/// - stores the new password's hash;
/// - verifies the address if it wasn't yet: the link proves its owner received it, as the
///   verification link would (AUTH-02);
/// - uses up the link, and every other reset link of the account;
/// - revokes all of the account's sign-ins (AUTH-07), and deletes its pending sign-ins, which were
///   started with the old password;
/// - lifts the block on sign-in with the address, if there is one (AUTH-08), so that the owner can
///   sign in with the new password at once. Codes stay blocked: the reset doesn't prove the second
///   factor.
///
/// It records `password_reset_completed`, `email_verified` if the address wasn't, and
/// `sign_in_session_revoked` for each sign-in, by the account's owner. It doesn't sign in: with
/// MFA on, signing in still takes a code.
pub(super) async fn complete(
    State(accounts): State<Arc<Accounts>>,
    CorrelationId(correlation_id): CorrelationId,
    ClientOrigin(origin): ClientOrigin,
    request: Result<Json<CompleteRequest>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(request) = request?;
    if find_link(&accounts.db, &request.token, false)
        .await?
        .is_none()
    {
        return Err(EXPIRED);
    }
    if request.password.char_count() == 0 {
        return Err(ApiError::Invalid(FieldErrors::from([(
            "password",
            FieldError::new("required"),
        )])));
    }
    // Before the transaction, so that no row stays locked while it is hashed.
    let password_hash = match accounts.passwords.hash_new(&request.password).await {
        Ok(hash) => hash,
        Err(NewPasswordError::Rejected(rejected)) => {
            return Err(ApiError::Invalid(sign_up::password_error(rejected)));
        }
        Err(NewPasswordError::Hashing(err)) => return Err(ApiError::internal(err)),
    };

    let txn = accounts.db.begin().await?;
    // Locked, so that two requests with the same link cannot both use it.
    let Some((_, user)) = find_link(&txn, &request.token, true).await? else {
        return Err(EXPIRED);
    };
    let now = Utc::now();
    let user_id = user.id;
    let newly_verified = !user.is_verified();
    let subject = throttle::subject(user.email.as_str());
    let mut user = user.into_active_model();
    user.password_hash = Set(password_hash);
    if newly_verified {
        user.email_verified_at = Set(Some(now));
    }
    user.update(&txn).await?;
    // None of the account's reset links works any more, so they all count as used.
    email_link::Entity::update_many()
        .set(email_link::ActiveModel {
            used_at: Set(Some(now)),
            ..Default::default()
        })
        .filter(email_link::Column::UserId.eq(user_id))
        .filter(email_link::Column::Purpose.eq(Purpose::ResetPassword))
        .filter(email_link::Column::UsedAt.is_null())
        .exec(&txn)
        .await?;

    let actor = Actor::User {
        id: user_id,
        origin,
    };
    let mut actions = vec![Action::PasswordResetCompleted];
    if newly_verified {
        actions.push(Action::EmailVerified);
    }
    for action in actions {
        audit::record(
            &txn,
            Event {
                actor: actor.clone(),
                action,
                target: Some(Target::User(user_id)),
                outcome: Outcome::Success,
                correlation_id,
                details: json!({}),
            },
        )
        .await?;
    }
    sign_in_session::revoke(
        &txn,
        user_id,
        Revoke::All,
        Action::SignInSessionRevoked,
        actor,
        correlation_id,
    )
    .await?;
    pending_sign_in::remove_all(&txn, user_id).await?;
    throttle::clear(&txn, Scope::SignIn, &subject).await?;
    txn.commit().await?;
    Ok(StatusCode::NO_CONTENT)
}

/// The reset link with `token` and its account, if the link works: it hasn't been used or
/// expired. With `lock`, both stay locked until the transaction of `db` ends.
async fn find_link<C: ConnectionTrait>(
    db: &C,
    token: &str,
    lock: bool,
) -> Result<Option<(email_link::Model, user::Model)>, DbErr> {
    if token.len() > MAX_TOKEN_LENGTH {
        return Ok(None);
    }
    let mut link = email_link::Entity::find()
        .filter(email_link::Column::TokenHash.eq(email_link::hash(token)))
        .filter(email_link::Column::Purpose.eq(Purpose::ResetPassword));
    if lock {
        link = link.lock_exclusive();
    }
    let Some(link) = link.one(db).await? else {
        return Ok(None);
    };
    if link.used_at.is_some() || link.expires_at <= Utc::now() {
        return Ok(None);
    }
    let mut user = user::Entity::find_by_id(link.user_id);
    if lock {
        user = user.lock_exclusive();
    }
    Ok(user.one(db).await?.map(|user| (link, user)))
}

impl Accounts {
    fn send_reset(&self, to: &EmailAddress, token: &Token) {
        // In the fragment, which browsers never send to a server, as for email verification.
        let link = format!(
            "{}#{}",
            self.public_url.join(RESET_PASSWORD_PAGE),
            token.expose()
        );
        self.mailer.send(
            to,
            &ResetPassword {
                link: &link,
                valid_for: self.password_reset.link_lifetime,
            },
        );
    }
}
