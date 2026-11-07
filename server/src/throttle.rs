//! Brute-force protection (SEC-08): after too many failures within a while, further attempts are
//! blocked for a while.
//!
//! A [`Rule`] says how many failures (`max_failures`) within a sliding `window` block a subject
//! for `block_for`. The subject is what is being guessed for, such as the email address at sign-in
//! (AUTH-08), and the [`Scope`] keeps the counts of features apart. Subjects are stored only as
//! SHA-256 hashes ([`subject`]), and a row is deleted once its failures have left the window and
//! its block has ended, so the table holds nothing for long.
//!
//! The counts are in the database, so every instance of the server (CAP-03) sees the same counts,
//! and a restart doesn't lift a block. A feature that checks a secret:
//!
//! 1. calls [`check`] first, and refuses the attempt while the subject is blocked, before doing any
//!    expensive work such as hashing a password;
//! 2. on a failure, calls [`record_failure`] in the transaction that records the failure's audit
//!    event. It returns [`Failure::BlockStarted`] for exactly one failure per block, even when
//!    attempts run concurrently, so that the feature can audit the block and notify someone once;
//! 3. on a success, calls [`clear_unless_blocked`] in the transaction of the success, which
//!    forgets the failures, unless a concurrent failure has just started a block.
//!
//! Attempts during a block aren't counted, so a block ends `block_for` after it started, however
//! often someone tries meanwhile. Only [`clear`] ends one early: a password reset (AUTH-07) lifts
//! the block on sign-in with the address, whose owner has just chosen a new password.

use std::time::Duration;

use chrono::{DateTime, Utc};
use sea_orm::{
    ActiveModelTrait, ConnectionTrait, DbErr, IntoActiveModel, QuerySelect, Set,
    entity::prelude::*,
    sea_query::{LockBehavior, LockType, OnConflict, Query},
};
use sha2::{Digest, Sha256};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "throttle")]
pub struct Model {
    #[sea_orm(
        primary_key,
        auto_increment = false,
        default_expr = "Expr::cust(\"gen_random_uuid()\")"
    )]
    pub id: Uuid,
    #[sea_orm(unique_key = "scope_subject")]
    pub scope: Scope,
    /// The SHA-256 of the subject ([`subject`]), never the subject itself.
    #[sea_orm(unique_key = "scope_subject")]
    pub subject: Vec<u8>,
    /// The failures within the window. Fewer than the rule's `max_failures`: reaching it starts a
    /// block, and the count starts again.
    pub failed_at: Vec<DateTimeUtc>,
    /// Until when attempts are refused; `None` without a block.
    pub blocked_until: Option<DateTimeUtc>,
    /// When the row no longer matters: its failures have left the window, and its block has
    /// ended. Expired rows are deleted.
    #[sea_orm(indexed)]
    pub expires_at: DateTimeUtc,
}

impl ActiveModelBehavior for ActiveModel {}

/// Which feature's counts a row holds. Stored as text: schema sync cannot add values to a
/// PostgreSQL enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(
    rs_type = "String",
    db_type = "String(StringLen::None)",
    rename_all = "snake_case"
)]
pub enum Scope {
    /// Passwords: at sign-in (AUTH-08), and at re-authentication (AUTH-10). The subject is the
    /// email address, whether or not it has an account, so that a block doesn't reveal whether it
    /// has one (SEC-14).
    SignIn,
    /// Codes from an authenticator app, and recovery codes, at the second step of sign-in and at
    /// re-authentication (SEC-08). The subject is the user's ID: only someone who knew the
    /// password gets this far.
    SecondFactor,
}

/// How many failures within how long block a subject, and for how long.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Rule {
    /// The failures within `window` that start a block. At least 1.
    pub max_failures: usize,
    pub window: Duration,
    pub block_for: Duration,
}

/// Whether a subject may make an attempt.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Status {
    Open,
    /// Not before `retry_after` from now.
    Blocked {
        retry_after: Duration,
    },
}

/// What [`record_failure`] did with a failure.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Failure {
    /// The failure was counted, and the subject isn't blocked.
    Counted,
    /// The failure reached `max_failures`, and the subject is now blocked for `block_for`.
    BlockStarted,
    /// The subject was already blocked by a concurrent attempt, so the failure wasn't counted.
    WhileBlocked,
}

/// The subject `key` as stored: its SHA-256.
pub fn subject(key: &str) -> Vec<u8> {
    Sha256::digest(key.as_bytes()).to_vec()
}

/// At most this many expired rows are deleted by each [`record_failure`]. Each failure adds at
/// most one row, so this keeps up.
const CLEANUP_BATCH: u64 = 100;

/// Whether `subject` in `scope` may make an attempt now. It doesn't lock anything: a concurrent
/// failure can start a block right after, which [`clear_unless_blocked`] catches.
pub async fn check<C: ConnectionTrait>(
    db: &C,
    scope: Scope,
    subject: &[u8],
) -> Result<Status, DbErr> {
    let stored = find(scope, subject).one(db).await?;
    Ok(stored.map_or(Status::Open, |row| State::of(&row).status(Utc::now())))
}

/// Counts a failure of `subject` in `scope` now, following `rule`. Call it in the transaction
/// that records the failure: the subject's row stays locked until it ends, so concurrent failures
/// are counted one after the other, and only one of them starts a block.
pub async fn record_failure<C: ConnectionTrait>(
    db: &C,
    scope: Scope,
    subject: &[u8],
    rule: &Rule,
) -> Result<Failure, DbErr> {
    let now = Utc::now();
    remove_expired(db, now).await?;
    // The update that does nothing locks the existing row, and returns it like a new one.
    let stored = Entity::insert(ActiveModel {
        scope: Set(scope),
        subject: Set(subject.to_vec()),
        failed_at: Set(Vec::new()),
        blocked_until: Set(None),
        expires_at: Set(now),
        ..Default::default()
    })
    .on_conflict(
        OnConflict::columns([Column::Scope, Column::Subject])
            .update_column(Column::Scope)
            .to_owned(),
    )
    .exec_with_returning(db)
    .await?;

    let mut state = State::of(&stored);
    let failure = state.fail(now, rule);
    if failure != Failure::WhileBlocked {
        let expires_at = state.expires_at(rule).max(now);
        let mut row = stored.into_active_model();
        row.failed_at = Set(state.failed_at);
        row.blocked_until = Set(state.blocked_until);
        row.expires_at = Set(expires_at);
        row.update(db).await?;
    }
    Ok(failure)
}

/// Forgets the failures of `subject` in `scope` after a success, and returns [`Status::Open`],
/// unless the subject is blocked, which a concurrent failure may have started since [`check`].
/// Then the success must be refused, and the status says until when.
pub async fn clear_unless_blocked<C: ConnectionTrait>(
    db: &C,
    scope: Scope,
    subject: &[u8],
) -> Result<Status, DbErr> {
    let Some(stored) = find(scope, subject).lock_exclusive().one(db).await? else {
        return Ok(Status::Open);
    };
    let status = State::of(&stored).status(Utc::now());
    if status == Status::Open {
        Entity::delete_by_id(stored.id).exec(db).await?;
    }
    Ok(status)
}

/// Forgets the failures of `subject` in `scope`, and lifts its block if it has one.
pub async fn clear<C: ConnectionTrait>(db: &C, scope: Scope, subject: &[u8]) -> Result<(), DbErr> {
    Entity::delete_many()
        .filter(Column::Scope.eq(scope))
        .filter(Column::Subject.eq(subject.to_vec()))
        .exec(db)
        .await?;
    Ok(())
}

fn find(scope: Scope, subject: &[u8]) -> Select<Entity> {
    Entity::find()
        .filter(Column::Scope.eq(scope))
        .filter(Column::Subject.eq(subject.to_vec()))
}

/// Deletes some expired rows. Rows that another transaction has locked are skipped rather than
/// waited for, so concurrent cleanups never wait for or deadlock with each other.
async fn remove_expired<C: ConnectionTrait>(db: &C, now: DateTime<Utc>) -> Result<(), DbErr> {
    let expired = Query::select()
        .column(Column::Id)
        .from(Entity)
        .and_where(Column::ExpiresAt.lte(now))
        .limit(CLEANUP_BATCH)
        .lock_with_behavior(LockType::Update, LockBehavior::SkipLocked)
        .to_owned();
    Entity::delete_many()
        .filter(Column::Id.in_subquery(expired))
        .exec(db)
        .await?;
    Ok(())
}

/// `duration` after `time`, or the latest time there is if that is later.
fn after(time: DateTime<Utc>, duration: Duration) -> DateTime<Utc> {
    chrono::Duration::from_std(duration)
        .ok()
        .and_then(|duration| time.checked_add_signed(duration))
        .unwrap_or(DateTime::<Utc>::MAX_UTC)
}

/// The counts of one subject, apart from the database.
#[derive(Clone, Debug, Default, PartialEq, Eq)]
struct State {
    failed_at: Vec<DateTime<Utc>>,
    blocked_until: Option<DateTime<Utc>>,
}

impl State {
    fn of(row: &Model) -> Self {
        Self {
            failed_at: row.failed_at.clone(),
            blocked_until: row.blocked_until,
        }
    }

    fn status(&self, now: DateTime<Utc>) -> Status {
        match self.blocked_until {
            Some(until) if until > now => Status::Blocked {
                retry_after: (until - now).to_std().unwrap_or_default(),
            },
            _ => Status::Open,
        }
    }

    /// Counts a failure at `now`.
    fn fail(&mut self, now: DateTime<Utc>, rule: &Rule) -> Failure {
        if self.status(now) != Status::Open {
            return Failure::WhileBlocked;
        }
        self.blocked_until = None;
        // A failure `window` old has left the window.
        self.failed_at.retain(|&at| after(at, rule.window) > now);
        self.failed_at.push(now);
        if self.failed_at.len() >= rule.max_failures {
            self.failed_at.clear();
            self.blocked_until = Some(after(now, rule.block_for));
            Failure::BlockStarted
        } else {
            Failure::Counted
        }
    }

    /// When the counts stop mattering: the last failure has left the window, and the block has
    /// ended. The earliest time there is when there is neither.
    fn expires_at(&self, rule: &Rule) -> DateTime<Utc> {
        self.failed_at
            .iter()
            .map(|&at| after(at, rule.window))
            .chain(self.blocked_until)
            .max()
            .unwrap_or(DateTime::<Utc>::MIN_UTC)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const RULE: Rule = Rule {
        max_failures: 3,
        window: Duration::from_secs(60),
        block_for: Duration::from_secs(300),
    };

    fn at(seconds: i64) -> DateTime<Utc> {
        DateTime::from_timestamp(1_800_000_000 + seconds, 0).unwrap()
    }

    #[test]
    fn max_failures_within_the_window_start_a_block() {
        let mut state = State::default();

        assert_eq!(state.fail(at(0), &RULE), Failure::Counted);
        assert_eq!(state.fail(at(30), &RULE), Failure::Counted);
        assert_eq!(state.status(at(59)), Status::Open);
        assert_eq!(state.fail(at(59), &RULE), Failure::BlockStarted);

        assert_eq!(
            state.status(at(60)),
            Status::Blocked {
                retry_after: Duration::from_secs(299)
            }
        );
        assert_eq!(state.blocked_until, Some(at(359)));
        // The count starts again, so the block's row expires when the block ends.
        assert!(state.failed_at.is_empty());
        assert_eq!(state.expires_at(&RULE), at(359));
    }

    #[test]
    fn the_window_slides() {
        let mut state = State::default();

        assert_eq!(state.fail(at(0), &RULE), Failure::Counted);
        assert_eq!(state.fail(at(40), &RULE), Failure::Counted);
        // The first failure is exactly a window old, so it no longer counts.
        assert_eq!(state.fail(at(60), &RULE), Failure::Counted);
        assert_eq!(state.failed_at, [at(40), at(60)]);
        assert_eq!(state.expires_at(&RULE), at(120));
        assert_eq!(state.fail(at(99), &RULE), Failure::BlockStarted);
    }

    #[test]
    fn failures_during_a_block_are_not_counted() {
        let mut state = State::default();
        for second in 0..3 {
            state.fail(at(second), &RULE);
        }
        let blocked = state.clone();

        assert_eq!(state.fail(at(100), &RULE), Failure::WhileBlocked);
        assert_eq!(state.fail(at(301), &RULE), Failure::WhileBlocked);
        assert_eq!(state, blocked);

        // Once the block has ended, counting starts again.
        assert_eq!(state.status(at(302)), Status::Open);
        assert_eq!(state.fail(at(302), &RULE), Failure::Counted);
        assert_eq!(state.blocked_until, None);
        assert_eq!(state.failed_at, [at(302)]);
    }

    #[test]
    fn one_failure_can_be_enough() {
        let mut state = State::default();

        assert_eq!(
            state.fail(
                at(0),
                &Rule {
                    max_failures: 1,
                    ..RULE
                }
            ),
            Failure::BlockStarted
        );
    }

    #[test]
    fn huge_durations_do_not_overflow() {
        let rule = Rule {
            max_failures: 1,
            window: Duration::MAX,
            block_for: Duration::MAX,
        };
        let mut state = State::default();

        assert_eq!(state.fail(at(0), &rule), Failure::BlockStarted);
        assert_eq!(state.expires_at(&rule), DateTime::<Utc>::MAX_UTC);
        assert!(matches!(state.status(at(1)), Status::Blocked { .. }));
    }

    #[test]
    fn subjects_are_hashed() {
        let hashed = subject("ann@example.com");

        assert_eq!(hashed.len(), 32);
        assert_ne!(hashed, subject("bob@example.com"));
        assert_eq!(hashed, subject("ann@example.com"));
    }
}
