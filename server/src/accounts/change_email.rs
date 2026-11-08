//! Changing the email address of an account (AUTH-11). The change takes effect only once the new
//! address is verified, with a single-use link emailed to it, and the old address is then told.
//!
//! - `POST /api/account/email` ([`request`]) emails the link. It needs a recent authentication
//!   (AUTH-10).
//! - `POST /api/account/email/confirm` ([`confirm`]) changes the address with the token from the
//!   link. It needs no sign-in, since the link may be opened on another device: the token shows
//!   that its holder reads the new address's mail, and the request showed the rest.
//!
//! Requests are limited per account ([`super::limits`]), so that an account cannot be used to send
//! email to many addresses. Links carry a 256-bit token, which cannot be guessed, so confirming
//! needs no limit of its own.

use std::{sync::Arc, time::Instant};

use axum::{
    Json,
    extract::{State, rejection::JsonRejection},
    http::StatusCode,
};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ColumnTrait, ConnectionTrait, EntityTrait, IntoActiveModel, QueryFilter,
    QuerySelect, Set, SqlErr, TransactionTrait, prelude::Uuid,
};
use serde::{Deserialize, Serialize};
use serde_json::json;

use super::{
    Accounts, CONFIRM_EMAIL_PAGE, RESET_PASSWORD_PAGE, RecentlyAuthenticated, SETTINGS_PAGE,
    SIGN_IN_PAGE, account,
    email_link::{self, MAX_TOKEN_LENGTH, Purpose, Token},
    sign_up,
    signed_in::NOT_SIGNED_IN,
    user,
};
use crate::{
    api::{ApiError, FieldError, FieldErrors},
    audit::{self, Action, Actor, Event, Origin, Outcome, Target},
    db::EmailAddress,
    email::{ConfirmEmail, EmailChanged, EmailInUse},
    http::{ClientOrigin, CorrelationId},
};

/// The answer to a link that has been used or has expired, or never existed.
const EXPIRED: ApiError = ApiError::Gone {
    code: "link_expired",
};

/// The answer to a link whose address has got an account of its own since it was sent.
const EMAIL_IN_USE: ApiError = ApiError::Conflict {
    code: "email_in_use",
};

/// The reason code of a request refused because the account asked too often.
const TOO_MANY_REQUESTS: &str = "too_many_requests";

#[derive(Deserialize)]
pub(super) struct ChangeRequest {
    email: String,
}

/// Emails a link that changes the signed-in user's address to `email`, and answers `202 Accepted`
/// with no body. `422` for something that isn't an address, or the address the account already
/// has (`unchanged`); `429` once the account has asked `email_change.max_per_account` times within
/// `email_change.window`.
///
/// The request stores a link, which works for `accounts.verification_link_lifetime`, and the
/// account's earlier change links stop working, so only the newest change can happen. Then:
///
/// - A free address gets the link.
/// - An address with an account gets an email that says so instead. Its link is never sent, so it
///   cannot be used.
///
/// Everything the signed-in user can see is the same either way, so that being signed in doesn't
/// let anyone find out who is registered: the answer, the change waiting in `GET /api/account`,
/// the audit event `email_change_requested` (AUD-07), and the count toward the limit.
pub(super) async fn request(
    State(accounts): State<Arc<Accounts>>,
    CorrelationId(correlation_id): CorrelationId,
    ClientOrigin(origin): ClientOrigin,
    RecentlyAuthenticated(signed_in): RecentlyAuthenticated,
    request: Result<Json<ChangeRequest>, JsonRejection>,
) -> Result<StatusCode, ApiError> {
    let Json(request) = request?;
    let invalid = |error| ApiError::Invalid(FieldErrors::from([("email", error)]));
    let new_email = sign_up::parse_email(&request.email).map_err(invalid)?;
    let user_id = signed_in.user_id;
    let requested = Requested {
        user_id,
        origin: &origin,
        correlation_id,
    };

    let txn = accounts.db.begin().await?;
    // Locked, so that a concurrent change of address doesn't slip between the check and the link.
    let user = user::Entity::find_by_id(user_id)
        .lock_exclusive()
        .one(&txn)
        .await?
        .ok_or(NOT_SIGNED_IN)?;
    if user.email == new_email {
        return Err(invalid(FieldError::new("unchanged")));
    }
    if let Err(retry_after) = accounts.limits.admit_email_change(user_id, Instant::now()) {
        // Nothing changes, but the event is written before the answer (AUD-03).
        requested
            .record(
                &txn,
                Outcome::Failure {
                    reason_code: TOO_MANY_REQUESTS,
                },
            )
            .await?;
        txn.commit().await?;
        return Err(ApiError::TooManyRequests { retry_after });
    }

    let taken = user::Entity::find()
        .filter(user::Column::Email.eq(new_email.clone()))
        .one(&txn)
        .await?
        .is_some();
    email_link::use_up(&txn, user_id, Some(Purpose::ChangeEmail)).await?;
    let token = email_link::create(
        &txn,
        user_id,
        Purpose::ChangeEmail,
        Some(new_email.clone()),
        accounts.config.verification_link_lifetime,
    )
    .await?;
    // Open settings show the change waiting to be confirmed.
    account::changed(&txn, user_id).await?;
    requested.record(&txn, Outcome::Success).await?;
    txn.commit().await?;

    if taken {
        let (sign_in_url, reset_url) = (
            accounts.public_url.join(SIGN_IN_PAGE),
            accounts.public_url.join(RESET_PASSWORD_PAGE),
        );
        accounts.mailer.send(
            &new_email,
            &EmailInUse {
                sign_in_url: &sign_in_url,
                reset_url: &reset_url,
            },
        );
    } else {
        accounts.send_confirmation(&new_email, &token);
    }
    Ok(StatusCode::ACCEPTED)
}

/// The event of one change request.
struct Requested<'a> {
    user_id: Uuid,
    origin: &'a Origin,
    correlation_id: Uuid,
}

impl Requested<'_> {
    async fn record<C: ConnectionTrait>(
        &self,
        db: &C,
        outcome: Outcome,
    ) -> Result<(), audit::Error> {
        audit::record(
            db,
            Event {
                actor: Actor::User {
                    id: self.user_id,
                    origin: self.origin.clone(),
                },
                action: Action::EmailChangeRequested,
                target: Some(Target::User(self.user_id)),
                outcome,
                correlation_id: self.correlation_id,
                // Neither address, nor whether the new one was free: the user reads these.
                details: json!({}),
            },
        )
        .await?;
        Ok(())
    }
}

#[derive(Deserialize)]
pub(super) struct ConfirmRequest {
    /// The token from the link. It is a secret: never log it.
    token: String,
}

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
pub(super) struct Confirmed {
    /// The account's address, now the one the link was sent to.
    email: String,
}

/// Changes the address of the link's account to the one the link was sent to, and answers
/// `200 OK` with it. Opening the link again answers the same while the account has that address:
/// email scanners often open links before people do. Any other link that has been used or has
/// expired, or never existed, answers `410 Gone`; and `409` (`email_in_use`) if the address has
/// got an account of its own since the link was sent.
///
/// In one transaction, it:
///
/// - changes the address, and marks it verified: the link proves that its owner received it;
/// - uses up every link of the account, of every purpose: they went to the old address, or were
///   for other changes, and whoever reads the old address's mail must not be able to use them;
/// - tells the account's browsers, so that open settings show the new address.
///
/// It records `email_change_completed`, and `email_verified` if the account wasn't verified yet,
/// by the account's owner. Then it tells the old address, if it was verified: one that never was
/// may belong to someone else.
pub(super) async fn confirm(
    State(accounts): State<Arc<Accounts>>,
    CorrelationId(correlation_id): CorrelationId,
    ClientOrigin(origin): ClientOrigin,
    request: Result<Json<ConfirmRequest>, JsonRejection>,
) -> Result<Json<Confirmed>, ApiError> {
    let Json(request) = request?;
    if request.token.len() > MAX_TOKEN_LENGTH {
        return Err(EXPIRED);
    }

    let txn = accounts.db.begin().await?;
    // Locked, so that two requests with the same link cannot both use it.
    let Some(link) = email_link::Entity::find()
        .filter(email_link::Column::TokenHash.eq(email_link::hash(&request.token)))
        .filter(email_link::Column::Purpose.eq(Purpose::ChangeEmail))
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
    let Some(new_email) = link.new_email.clone() else {
        // The database never holds such a link: `create` requires the address.
        return Err(EXPIRED);
    };
    if link.used_at.is_some() && user.email == new_email {
        return Ok(Json(Confirmed {
            email: new_email.as_str().to_owned(),
        }));
    }
    let now = Utc::now();
    if link.used_at.is_some() || link.expires_at <= now {
        return Err(EXPIRED);
    }

    let user_id = user.id;
    let old_email = user.email.clone();
    let old_verified = user.is_verified();
    let mut user = user.into_active_model();
    user.email = Set(new_email.clone());
    user.email_verified_at = Set(Some(now));
    if let Err(err) = user.update(&txn).await {
        if matches!(err.sql_err(), Some(SqlErr::UniqueConstraintViolation(_))) {
            txn.rollback().await?;
            return Err(EMAIL_IN_USE);
        }
        return Err(err.into());
    }
    email_link::use_up(&txn, user_id, None).await?;

    let actor = Actor::User {
        id: user_id,
        origin,
    };
    let mut actions = vec![Action::EmailChangeCompleted];
    if !old_verified {
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
    account::changed(&txn, user_id).await?;
    txn.commit().await?;

    if old_verified {
        let settings_url = accounts.public_url.join(SETTINGS_PAGE);
        accounts.mailer.send(
            &old_email,
            &EmailChanged {
                new_email: new_email.as_str(),
                settings_url: &settings_url,
            },
        );
    }
    Ok(Json(Confirmed {
        email: new_email.as_str().to_owned(),
    }))
}

impl Accounts {
    fn send_confirmation(&self, to: &EmailAddress, token: &Token) {
        // In the fragment, which browsers never send to a server, as for email verification.
        let link = format!(
            "{}#{}",
            self.public_url.join(CONFIRM_EMAIL_PAGE),
            token.expose()
        );
        self.mailer.send(
            to,
            &ConfirmEmail {
                link: &link,
                valid_for: self.config.verification_link_lifetime,
            },
        );
    }
}
