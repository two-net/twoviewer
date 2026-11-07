//! Browser sign-ins (SignInSession in §5.2).
//!
//! Signing in creates one, and the browser holds its [`Token`](super::token::Token) in a cookie
//! ([`super::signed_in`]). The database stores only the token's hash (SEC-07), so a copy of the
//! database cannot be used to sign in.
//!
//! A sign-in ends after `sign_in.idle_timeout` without use, and at the latest
//! `sign_in.session_lifetime` after it started (AUTH-09). `expires_at` is when it ends unless it is
//! used again: [`Lifetime::expiry`]. Every request that it authenticates records its activity and
//! moves `expires_at` on ([`super::signed_in`]).
//!
//! It also ends when it is revoked, with [`revoke`]: by signing out, by the user from their list
//! of sign-ins, and for all of a user's sign-ins by password reset (AUTH-07), account deletion
//! (AUTH-14) and suspension (OPS-03). Revoking closes the browser's real-time connection.
//!
//! Sign-ins are kept for 30 days after they expire or are revoked, that is after
//! `coalesce(revoked_at, expires_at)`, then deleted by the retention job (Table 5.3-A, #66).

use std::time::Duration;

use chrono::{DateTime, Utc};
use sea_orm::{ConnectionTrait, QueryFilter, Set, entity::prelude::*};
use serde_json::json;
use twoviewer_protocol::Event;

use crate::{
    audit::{self, Action, Actor, Outcome, Target},
    config,
    realtime::{self, Topic},
};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "sign_in_session")]
pub struct Model {
    #[sea_orm(
        primary_key,
        auto_increment = false,
        default_expr = "Expr::cust(\"gen_random_uuid()\")"
    )]
    pub id: Uuid,
    #[sea_orm(indexed)]
    pub user_id: Uuid,
    /// SHA-256 of the token in the cookie, never the token itself (SEC-07).
    #[sea_orm(unique)]
    pub token_hash: Vec<u8>,
    #[sea_orm(default_expr = "Expr::current_timestamp()")]
    pub created_at: DateTimeUtc,
    /// When the sign-in was last used, to the minute ([`super::signed_in`]).
    pub last_active_at: DateTimeUtc,
    /// When the sign-in ends unless it is used again ([`Lifetime::expiry`]).
    pub expires_at: DateTimeUtc,
    /// When the user last entered their password for this sign-in, and their second factor if
    /// MFA is on: at sign-in, or when they last re-authenticated. Sensitive changes need it to be
    /// recent (AUTH-10, [`super::signed_in::RecentlyAuthenticated`]).
    pub authenticated_at: DateTimeUtc,
    /// Where the user signed in from, which they see in their list of sign-ins (AUTH-09).
    pub ip_address: IpNetwork,
    /// Browser and operating system, such as "Firefox on Linux".
    pub client_description: String,
    /// When the sign-in was revoked; `None` while it isn't.
    pub revoked_at: Option<DateTimeUtc>,
    #[sea_orm(belongs_to, from = "user_id", to = "id", on_delete = "Cascade")]
    pub user: BelongsTo<super::user::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}

/// How long sign-ins last (AUTH-09).
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Lifetime {
    /// At most this long after sign-in, however much it is used.
    pub max: Duration,
    /// At most this long after it was last used.
    pub idle: Duration,
}

impl Lifetime {
    pub fn new(config: &config::SignIn) -> Self {
        Self {
            max: config.session_lifetime,
            idle: config.idle_timeout,
        }
    }

    /// When a sign-in made at `created_at` and last used at `last_active_at` ends, unless it is
    /// used again.
    pub fn expiry(&self, created_at: DateTimeUtc, last_active_at: DateTimeUtc) -> DateTimeUtc {
        after(created_at, self.max).min(after(last_active_at, self.idle))
    }
}

/// `duration` after `time`, or the end of time if that is further than it can say.
fn after(time: DateTimeUtc, duration: Duration) -> DateTimeUtc {
    chrono::Duration::from_std(duration)
        .ok()
        .and_then(|duration| time.checked_add_signed(duration))
        .unwrap_or(DateTime::<Utc>::MAX_UTC)
}

/// The event that tells a user's browsers that their sign-ins have changed, so that open lists of
/// sign-ins reload.
pub const CHANGED_EVENT: &str = "sign_ins.changed";

/// Tells `user_id`'s browsers that their sign-ins have changed, once the transaction of `db`
/// commits.
pub async fn changed<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
) -> Result<(), realtime::PublishError> {
    realtime::publish(
        db,
        Topic::User(user_id),
        &Event {
            name: CHANGED_EVENT.to_owned(),
            data: json!({}),
        },
    )
    .await
}

/// Which of a user's sign-ins to revoke.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Revoke {
    One(Uuid),
    /// All but this one, the sign-in of the request.
    AllExcept(Uuid),
    All,
}

#[derive(Debug, thiserror::Error)]
pub enum RevokeError {
    #[error("database error: {0}")]
    Database(#[from] DbErr),
    #[error("cannot record the audit event: {0}")]
    Audit(#[from] audit::Error),
    #[error("cannot end the real-time connection: {0}")]
    Realtime(#[from] realtime::PublishError),
}

/// Revokes `which` of `user_id`'s active sign-ins, and returns the IDs of those it revoked: none
/// for one that has already ended or isn't the user's.
///
/// Each revoked sign-in records an audit event of `action`, by `actor`, with its ID; and its
/// browser's real-time connection is closed. Pass the transaction the revocation belongs to
/// (AUD-03): the connections close once it commits.
pub async fn revoke<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    which: Revoke,
    action: Action,
    actor: Actor,
    correlation_id: Uuid,
) -> Result<Vec<Uuid>, RevokeError> {
    let now = Utc::now();
    let query = Entity::update_many()
        .set(ActiveModel {
            revoked_at: Set(Some(now)),
            ..Default::default()
        })
        .filter(Column::UserId.eq(user_id))
        .filter(Column::RevokedAt.is_null())
        .filter(Column::ExpiresAt.gt(now));
    let query = match which {
        Revoke::One(id) => query.filter(Column::Id.eq(id)),
        Revoke::AllExcept(id) => query.filter(Column::Id.ne(id)),
        Revoke::All => query,
    };
    let revoked: Vec<Uuid> = query
        .exec_with_returning(db)
        .await?
        .into_iter()
        .map(|session| session.id)
        .collect();

    for &id in &revoked {
        audit::record(
            db,
            audit::Event {
                actor: actor.clone(),
                action,
                target: Some(Target::User(user_id)),
                outcome: Outcome::Success,
                correlation_id,
                details: json!({ "sign_in_session_id": id }),
            },
        )
        .await?;
        realtime::end_sign_in(db, id).await?;
    }
    if !revoked.is_empty() {
        changed(db, user_id).await?;
    }
    Ok(revoked)
}

#[cfg(test)]
mod tests {
    use super::*;

    const DAY: Duration = Duration::from_secs(24 * 60 * 60);

    fn at(rfc3339: &str) -> DateTimeUtc {
        rfc3339.parse().unwrap()
    }

    #[test]
    fn sign_ins_end_after_inactivity_and_at_most_a_lifetime_after_sign_in() {
        let lifetime = Lifetime {
            max: 30 * DAY,
            idle: 7 * DAY,
        };
        let signed_in = at("2026-11-01T12:00:00Z");

        // Unused, it lasts the idle timeout.
        assert_eq!(
            lifetime.expiry(signed_in, signed_in),
            at("2026-11-08T12:00:00Z")
        );
        // Each use moves the end on.
        assert_eq!(
            lifetime.expiry(signed_in, at("2026-11-20T08:00:00Z")),
            at("2026-11-27T08:00:00Z")
        );
        // But never past the lifetime.
        assert_eq!(
            lifetime.expiry(signed_in, at("2026-11-28T08:00:00Z")),
            at("2026-12-01T12:00:00Z")
        );
    }

    #[test]
    fn lifetimes_too_long_to_say_end_at_the_end_of_time() {
        let lifetime = Lifetime {
            max: Duration::MAX,
            idle: Duration::MAX,
        };
        let now = Utc::now();

        assert_eq!(lifetime.expiry(now, now), DateTime::<Utc>::MAX_UTC);
    }
}
