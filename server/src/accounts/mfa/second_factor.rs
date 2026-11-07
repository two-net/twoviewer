//! Checking a second factor: a code from the authenticator app, or an unused recovery code. The
//! second step of sign-in (AUTH-05) and re-authentication (AUTH-10) both check them here, so that
//! both are throttled alike (SEC-08) and use codes up alike.
//!
//! A caller:
//!
//! 1. refuses the attempt while [`blocked`] says so, before any other work;
//! 2. calls [`verify`] in a transaction, which uses the code up if it is right;
//! 3. if it was right, calls [`succeeded`] in that transaction, which forgets earlier failures and
//!    audits the use of a recovery code. If a concurrent failure has blocked codes meanwhile, it
//!    rolls the transaction back instead, so that the code isn't used up, and refuses;
//! 4. if it was wrong, calls [`failed`] in that transaction, with its own audit event.
//!
//! Wrong codes are counted per user, in the database, like failed sign-ins ([`crate::throttle`]):
//! after `mfa.max_failures` within `mfa.failure_window`, codes are refused for
//! `mfa.block_duration`. With 6 digits and a window of three steps, a guess is right with a
//! chance of 3 in a million, so the default of 5 per 15 minutes keeps guessing hopeless.

use std::{fmt, time::Duration};

use chrono::Utc;
use sea_orm::{
    ColumnTrait, Condition, ConnectionTrait, DbErr, EntityTrait, QueryFilter, Set, prelude::Uuid,
};
use serde::Deserialize;
use serde_json::json;
use zeroize::Zeroizing;

use super::{cipher::Cipher, factor, recovery_code, totp};
use crate::{
    api::ApiError,
    audit::{self, Action, Actor, Event, Outcome, Target},
    config,
    throttle::{self, Failure, Scope, Status},
};

/// The reason code of the `sign_in_blocked` event of the failure that blocks a user's codes.
const TOO_MANY_FAILURES: &str = "too_many_second_factor_failures";

/// A code as typed: from the authenticator app, or a recovery code.
///
/// Its memory is cleared when it is dropped. It never appears in logs: `Debug` hides it, and
/// there is no `Display`. Request bodies can deserialize it directly from a string.
#[derive(Clone, Default, Deserialize)]
#[serde(from = "String")]
pub struct TypedCode(Zeroizing<String>);

impl TypedCode {
    pub fn new(typed: &str) -> Self {
        Self(Zeroizing::new(typed.to_owned()))
    }

    /// Whether nothing but spaces was typed.
    pub fn is_blank(&self) -> bool {
        self.0.trim().is_empty()
    }

    /// The code as typed. Never log it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl From<String> for TypedCode {
    fn from(typed: String) -> Self {
        Self(Zeroizing::new(typed))
    }
}

impl fmt::Debug for TypedCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("TypedCode(..)")
    }
}

/// Which second factor a right code was.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Method {
    Authenticator,
    Recovery,
}

impl Method {
    /// The name that audit events record.
    pub fn name(self) -> &'static str {
        match self {
            Method::Authenticator => "authenticator",
            Method::Recovery => "recovery",
        }
    }
}

/// How wrong codes are throttled.
pub fn rule(config: &config::Mfa) -> throttle::Rule {
    throttle::Rule {
        max_failures: config.max_failures,
        window: config.failure_window,
        block_for: config.block_duration,
    }
}

/// The throttle's subject for `user_id`'s codes.
fn subject(user_id: Uuid) -> Vec<u8> {
    throttle::subject(&user_id.to_string())
}

/// How long `user_id`'s codes stay refused, if they are.
pub async fn blocked<C: ConnectionTrait>(db: &C, user_id: Uuid) -> Result<Option<Duration>, DbErr> {
    Ok(
        match throttle::check(db, Scope::SecondFactor, &subject(user_id)).await? {
            Status::Blocked { retry_after } => Some(retry_after),
            Status::Open => None,
        },
    )
}

/// The confirmed factor of `user_id`, if MFA is on.
pub async fn enabled<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
) -> Result<Option<factor::Model>, DbErr> {
    factor::Entity::find_by_id(user_id)
        .filter(factor::Column::ConfirmedAt.is_not_null())
        .one(db)
        .await
}

/// Checks `typed` against `factor`, and uses it up if it is right: a code from the app can't be
/// used again, and neither can the codes before it; a recovery code is used once. Call it in the
/// transaction of the attempt.
pub async fn verify<C: ConnectionTrait>(
    db: &C,
    cipher: &Cipher,
    config: &config::Mfa,
    factor: &factor::Model,
    typed: &TypedCode,
) -> Result<Option<Method>, ApiError> {
    let user_id = factor.user_id;
    if !totp::is_code(typed.expose()) {
        let used = recovery_code::use_up(db, user_id, typed.expose()).await?;
        return Ok(used.then_some(Method::Recovery));
    }
    let secret = cipher
        .decrypt(user_id, &factor.secret_encrypted)
        .map_err(ApiError::internal)?;
    let Some(step) = secret.verify(
        typed.expose(),
        Utc::now(),
        config.clock_skew_steps,
        factor.last_used_step,
    ) else {
        return Ok(None);
    };
    // Only if no concurrent attempt has used this step or a later one in the meantime.
    let used = factor::Entity::update_many()
        .set(factor::ActiveModel {
            last_used_step: Set(Some(step)),
            ..Default::default()
        })
        .filter(factor::Column::UserId.eq(user_id))
        .filter(
            Condition::any()
                .add(factor::Column::LastUsedStep.is_null())
                .add(factor::Column::LastUsedStep.lt(step)),
        )
        .exec(db)
        .await?;
    Ok((used.rows_affected == 1).then_some(Method::Authenticator))
}

/// After [`verify`] found a right code of `user_id`, in the same transaction: forgets earlier
/// wrong codes, and records `recovery_code_used` by `actor` if it was a recovery code. Returns
/// [`Status::Blocked`] if a concurrent failure has blocked codes meanwhile: then roll the
/// transaction back, and refuse.
pub async fn succeeded<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    method: Method,
    actor: Actor,
    correlation_id: Uuid,
) -> Result<Status, ApiError> {
    let status = throttle::clear_unless_blocked(db, Scope::SecondFactor, &subject(user_id)).await?;
    if status == Status::Open && method == Method::Recovery {
        let remaining = recovery_code::unused(db, user_id).await?;
        audit::record(
            db,
            Event {
                actor,
                action: Action::RecoveryCodeUsed,
                target: Some(Target::User(user_id)),
                outcome: Outcome::Success,
                correlation_id,
                details: json!({ "remaining": remaining }),
            },
        )
        .await?;
        // #32: email the user that a recovery code was used (AUTH-12).
    }
    Ok(status)
}

/// Counts a wrong code of `user_id`, and records `failure`, its audit event. When the failure
/// blocks the user's codes, also records `sign_in_blocked` by the same actor. Call it in the
/// transaction of the attempt.
pub async fn failed<C: ConnectionTrait>(
    db: &C,
    config: &config::Mfa,
    user_id: Uuid,
    failure: Event,
) -> Result<(), ApiError> {
    let counted =
        throttle::record_failure(db, Scope::SecondFactor, &subject(user_id), &rule(config)).await?;
    let blocked = Event {
        action: Action::SignInBlocked,
        outcome: Outcome::Failure {
            reason_code: TOO_MANY_FAILURES,
        },
        details: json!({ "blocked_seconds": config.block_duration.as_secs() }),
        ..failure.clone()
    };
    audit::record(db, failure).await?;
    if counted == Failure::BlockStarted {
        audit::record(db, blocked).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn typed_codes_are_not_formatted() {
        let code = TypedCode::new("123456");

        assert!(!format!("{code:?}").contains("123456"));
        assert!(TypedCode::new("  ").is_blank());
        assert!(!code.is_blank());
    }
}
