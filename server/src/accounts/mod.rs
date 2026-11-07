//! User accounts (§3.1).
//!
//! # Sign-up and email verification
//!
//! [`Accounts::routes`] serves the API of sign-up (AUTH-01, AUTH-15) and email verification
//! (AUTH-02):
//!
//! - `POST /api/sign-up` creates an unverified account, and emails a link that verifies the
//!   address. It answers `202`, the same whether or not the address is registered (SEC-14), or
//!   `422`, `409`, `428` or `429`.
//! - `POST /api/verify-email` verifies the address with the token from the link. It answers `204`,
//!   or `410` for a link that has expired or doesn't exist.
//!
//! [`sign_up::sign_up`] and [`verify_email::verify_email`] say what they do in detail, and
//! `server/README.md` says why.
//!
//! **Unverified accounts.** Until a user's address is verified, they can sign in but cannot pair
//! devices or start sessions (AUTH-02). Pairing (#36) and sessions (#42, #43) must refuse users
//! for whom [`user::Model::is_verified`] is false.
//!
//! # Sign-in
//!
//! - `POST /api/sign-in` signs in with an email address and a password (AUTH-05). It answers `204`
//!   with the cookie of a new sign-in, or `401`, the same whether the password is wrong or the
//!   address has no account (SEC-14), or `422` or `429`. Failed attempts are throttled per address
//!   (AUTH-08) by [`crate::throttle`], and per network (SEC-08).
//! - `GET /api/account` answers the signed-in user's account, or `401`.
//!
//! [`sign_in::sign_in`] says what it does in detail. Handlers that need a signed-in user take
//! [`SignedIn`], which reads the cookie ([`signed_in`]).
//!
//! # Sign-ins
//!
//! A sign-in ends after `sign_in.idle_timeout` (7 days) without use, and at the latest
//! `sign_in.session_lifetime` (30 days) after sign-in (AUTH-09, [`sign_in_session`]). The user
//! manages theirs ([`sign_ins`]):
//!
//! - `GET /api/sign-ins` lists the active ones: browser and operating system, IP address, when
//!   they started and were last used, and which is the one of the request.
//! - `DELETE /api/sign-ins/{id}` revokes one. It answers `204`, or `410` for one that has ended or
//!   isn't the user's.
//! - `DELETE /api/sign-ins` revokes all but the one of the request, and answers `204`.
//! - `POST /api/sign-out` ends the one of the request, and answers `204` with a cookie that
//!   removes it, also when it had already ended.
//!
//! **Revoking all of a user's sign-ins.** Password reset (AUTH-07, #28), account deletion
//! (AUTH-14, #30) and suspension (OPS-03, #68) do this, in the transaction of their change:
//!
//! ```ignore
//! sign_in_session::revoke(&txn, user_id, Revoke::All, Action::SignInSessionRevoked, actor, correlation_id)
//!     .await?;
//! ```
//!
//! It records an audit event per sign-in, and closes their browsers' real-time connections once
//! the transaction commits.
//!
//! # Passwords
//!
//! [`password::Passwords`] holds the rules for new passwords (AUTH-03, AUTH-04) and the only way
//! to hash one (SEC-07). Every place that sets a password, which is sign-up, password change and
//! password reset, gets its hash from [`Passwords::hash_new`](password::Passwords::hash_new),
//! which rejects passwords that break the rules:
//!
//! ```ignore
//! let hash = match passwords.hash_new(&request.password).await {
//!     Ok(hash) => hash,
//!     Err(NewPasswordError::Rejected(rejected)) => return Err(form_error(rejected)),
//!     Err(NewPasswordError::Hashing(err)) => return Err(err.into()),
//! };
//! user.password_hash = Set(hash);
//! ```
//!
//! Sign-in checks a password with [`Passwords::verify`](password::Passwords::verify), also when
//! no account has the address, so that the response takes as long either way (SEC-14). A hash made
//! with older settings comes back renewed, to be stored:
//!
//! ```ignore
//! match passwords.verify(&request.password, user.as_ref().map(|u| &u.password_hash)).await? {
//!     Verification::Match { rehashed } => {
//!         if let Some(hash) = rehashed {
//!             // Store `hash` as the user's password hash.
//!         }
//!     }
//!     Verification::Mismatch => return Err(sign_in_failed()),
//! }
//! ```
//!
//! [`sign_in::sign_in`] does this.
//!
//! Passwords and their hashes are never logged or audited (MNT-03, AUD-06): their `Debug` output
//! hides them, and they have no `Display`. Neither are the [tokens](token) of email links and
//! sign-ins.

mod account;
pub mod challenge;
pub mod compromised;
pub mod email_link;
pub mod legal;
mod limits;
pub mod password;
mod sign_in;
pub mod sign_in_session;
mod sign_ins;
mod sign_up;
pub mod signed_in;
pub mod token;
pub mod user;
mod verify_email;

use std::sync::Arc;

use axum::{
    Router,
    extract::FromRef,
    routing::{delete, get, post},
};
use sea_orm::DatabaseConnection;

pub use self::signed_in::{Authenticator, SignedIn};
use self::{limits::Limits, password::Passwords, sign_in_session::Lifetime};
use crate::{
    config::{self, PublicUrl},
    email::Mailer,
    metrics::Metrics,
    throttle,
};

/// The path of sign-up in the API.
pub const SIGN_UP_PATH: &str = "/api/sign-up";
/// The path of email verification in the API.
pub const VERIFY_EMAIL_PATH: &str = "/api/verify-email";
/// The path of sign-in in the API.
pub const SIGN_IN_PATH: &str = "/api/sign-in";
/// The path of the signed-in user's account in the API.
pub const ACCOUNT_PATH: &str = "/api/account";
/// The path of the signed-in user's sign-ins in the API.
pub const SIGN_INS_PATH: &str = "/api/sign-ins";
/// The path of one of the signed-in user's sign-ins in the API.
pub const SIGN_INS_ITEM_PATH: &str = "/api/sign-ins/{id}";
/// The path of sign-out in the API.
pub const SIGN_OUT_PATH: &str = "/api/sign-out";

/// The pages of the web client that emails link to (`web/src/app/app.routes.ts`).
const VERIFY_EMAIL_PAGE: &str = "/verify-email";
const SIGN_IN_PAGE: &str = "/sign-in";
const RESET_PASSWORD_PAGE: &str = "/reset-password";

/// What the account endpoints share.
pub struct Accounts {
    db: DatabaseConnection,
    mailer: Mailer,
    passwords: Passwords,
    /// Where the web client is, which links in emails point to.
    public_url: PublicUrl,
    config: config::Accounts,
    sign_up: config::SignUp,
    sign_in: config::SignIn,
    /// How long sign-ins last.
    lifetime: Lifetime,
    limits: Limits,
    metrics: Arc<Metrics>,
}

impl Accounts {
    /// Uses the `accounts`, `sign_up` and `sign_in` sections of `config`.
    pub fn new(
        db: DatabaseConnection,
        mailer: Mailer,
        passwords: Passwords,
        public_url: PublicUrl,
        config: &config::Config,
        metrics: Arc<Metrics>,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            mailer,
            passwords,
            public_url,
            config: config.accounts.clone(),
            sign_up: config.sign_up.clone(),
            sign_in: config.sign_in.clone(),
            lifetime: Lifetime::new(&config.sign_in),
            limits: Limits::new(&config.sign_up, &config.sign_in),
            metrics,
        })
    }

    /// The routes of the account endpoints. They need connection information
    /// ([`crate::http::ClientOrigin`]).
    pub fn routes(self: &Arc<Self>) -> Router {
        Router::new()
            .route(SIGN_UP_PATH, post(sign_up::sign_up))
            .route(VERIFY_EMAIL_PATH, post(verify_email::verify_email))
            .route(SIGN_IN_PATH, post(sign_in::sign_in))
            .route(ACCOUNT_PATH, get(account::account))
            .route(
                SIGN_INS_PATH,
                get(sign_ins::list).delete(sign_ins::revoke_others),
            )
            .route(SIGN_INS_ITEM_PATH, delete(sign_ins::revoke))
            .route(SIGN_OUT_PATH, post(sign_ins::sign_out))
            .with_state(self.clone())
    }

    /// How failed sign-ins with one address are throttled (AUTH-08).
    fn sign_in_rule(&self) -> throttle::Rule {
        throttle::Rule {
            max_failures: self.sign_in.max_failures,
            window: self.sign_in.failure_window,
            block_for: self.sign_in.block_duration,
        }
    }
}

impl FromRef<Arc<Accounts>> for Authenticator {
    fn from_ref(accounts: &Arc<Accounts>) -> Self {
        Authenticator::new(accounts.db.clone(), accounts.lifetime)
    }
}
