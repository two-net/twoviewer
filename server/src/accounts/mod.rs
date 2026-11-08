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
//! # Password reset
//!
//! People who forgot their password get a link by email that sets a new one (AUTH-07,
//! [`password_reset`]):
//!
//! - `POST /api/password-reset` emails the link. It answers `202`, the same whether or not the
//!   address is registered (SEC-14), or `422` or `429`.
//! - `POST /api/password-reset/check` answers `200` with the link's address while the link works,
//!   or `410`.
//! - `POST /api/password-reset/complete` sets the new password with the token from the link, and
//!   revokes all of the account's sign-ins. It answers `204`, `410` or `422`.
//!
//! # Account settings
//!
//! Users change their display name, email address and password themselves (AUTH-11):
//!
//! - `PATCH /api/account` changes the display name ([`account::update`]). It answers `204`, or
//!   `422`.
//! - `POST /api/account/email` emails a link to a new address ([`change_email::request`]). It
//!   needs a recent authentication, and answers `202`, the same whether or not the address has an
//!   account, or `422` or `429`. `GET /api/account` shows the address as `pendingEmail` until the
//!   link is used or expires.
//! - `POST /api/account/email/confirm` changes the address with the token from the link
//!   ([`change_email::confirm`]), and tells the old address. It answers `200` with the new
//!   address, `409` if it has got an account since, or `410`.
//! - `POST /api/account/password` changes the password, with the current one
//!   ([`change_password::change_password`]), and signs out everywhere else. It needs a recent
//!   authentication, and answers `204`, `422` or `429`.
//!
//! Each tells the user's browsers with the real-time event [`account::CHANGED_EVENT`] when the
//! account changes, and the password change with that of revoked sign-ins.
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
//! With MFA on, a right password answers `428` (`second_factor_required`) instead, with the cookie
//! of a [pending sign-in](pending_sign_in), and the second step completes it (AUTH-05):
//!
//! - `POST /api/sign-in/second-factor` takes a code from the authenticator app or an unused
//!   recovery code, and answers `204` with the cookie of the new sign-in, or `401`, `410` when
//!   the pending sign-in has expired, `422` or `429` ([`sign_in_second_factor::second_factor`]).
//!
//! # MFA
//!
//! Users turn MFA with an authenticator app on and off themselves (AUTH-06, [`mfa`]):
//!
//! - `GET /api/mfa` says whether it is on, and how many recovery codes are left.
//! - `POST /api/mfa/setup` stores a new secret and answers it, also as a QR code.
//! - `POST /api/mfa/confirm` turns MFA on with a code from the app, and answers 10 recovery codes,
//!   the only time they are shown.
//! - `DELETE /api/mfa` turns MFA off.
//!
//! # Re-authentication
//!
//! Sensitive changes need the password, and the second factor if MFA is on, to have been entered
//! within `sign_in.reauthenticate_after` (15 minutes, AUTH-10): changing the password, email
//! address or MFA settings, turning on unattended access, and deleting the account. Their handlers
//! take [`RecentlyAuthenticated`] instead of [`SignedIn`]:
//!
//! ```ignore
//! pub async fn change_password(
//!     State(accounts): State<Arc<Accounts>>,
//!     RecentlyAuthenticated(signed_in): RecentlyAuthenticated,
//!     ...
//! ) -> Result<StatusCode, ApiError>
//! ```
//!
//! It answers `428` (`reauthentication_required`) otherwise, with `secondFactor` saying whether a
//! code is needed too. The web client then asks for them, and sends them to
//! `POST /api/reauthenticate` ([`reauthenticate::reauthenticate`]), which answers `204`, `401`,
//! `422` or `429`, before trying the change again.
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
//! **Revoking all of a user's sign-ins.** Password reset (AUTH-07, [`password_reset`]) does this,
//! and account deletion (AUTH-14, #30) and suspension (OPS-03, #68) must, in the transaction of
//! their change:
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

pub mod account;
pub mod challenge;
mod change_email;
mod change_password;
pub mod compromised;
pub mod email_link;
pub mod legal;
mod limits;
pub mod mfa;
pub mod password;
mod password_reset;
pub mod pending_sign_in;
mod reauthenticate;
mod sign_in;
mod sign_in_second_factor;
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

pub use self::signed_in::{Authenticator, RecentlyAuthenticated, SignedIn};
use self::{limits::Limits, mfa::cipher::Cipher, password::Passwords, sign_in_session::Lifetime};
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
/// The path that emails a link to change the signed-in user's email address in the API.
pub const ACCOUNT_EMAIL_PATH: &str = "/api/account/email";
/// The path that changes an email address with the link from that email in the API.
pub const ACCOUNT_EMAIL_CONFIRM_PATH: &str = "/api/account/email/confirm";
/// The path that changes the signed-in user's password in the API.
pub const ACCOUNT_PASSWORD_PATH: &str = "/api/account/password";
/// The path of the signed-in user's sign-ins in the API.
pub const SIGN_INS_PATH: &str = "/api/sign-ins";
/// The path of one of the signed-in user's sign-ins in the API.
pub const SIGN_INS_ITEM_PATH: &str = "/api/sign-ins/{id}";
/// The path of sign-out in the API.
pub const SIGN_OUT_PATH: &str = "/api/sign-out";
/// The path of the second step of sign-in with MFA in the API.
pub const SIGN_IN_SECOND_FACTOR_PATH: &str = "/api/sign-in/second-factor";
/// The path of re-authentication in the API.
pub const REAUTHENTICATE_PATH: &str = "/api/reauthenticate";
/// The path that emails a password reset link in the API.
pub const PASSWORD_RESET_PATH: &str = "/api/password-reset";
/// The path that says whether a password reset link still works in the API.
pub const PASSWORD_RESET_CHECK_PATH: &str = "/api/password-reset/check";
/// The path that sets a new password with a password reset link in the API.
pub const PASSWORD_RESET_COMPLETE_PATH: &str = "/api/password-reset/complete";
/// The path of the signed-in user's MFA settings in the API.
pub const MFA_PATH: &str = "/api/mfa";
/// The path that starts setting up an authenticator app in the API.
pub const MFA_SETUP_PATH: &str = "/api/mfa/setup";
/// The path that confirms an authenticator app, which turns MFA on, in the API.
pub const MFA_CONFIRM_PATH: &str = "/api/mfa/confirm";

/// The pages of the web client that emails link to (`web/src/app/app.routes.ts`).
const VERIFY_EMAIL_PAGE: &str = "/verify-email";
const SIGN_IN_PAGE: &str = "/sign-in";
const RESET_PASSWORD_PAGE: &str = "/reset-password";
const CONFIRM_EMAIL_PAGE: &str = "/confirm-email";
const SETTINGS_PAGE: &str = "/settings";

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
    mfa: config::Mfa,
    password_reset: config::PasswordReset,
    /// Encrypts the secrets of authenticator apps (SEC-07).
    mfa_cipher: Cipher,
    /// How long sign-ins last.
    lifetime: Lifetime,
    limits: Limits,
    metrics: Arc<Metrics>,
}

impl Accounts {
    /// Uses the `accounts`, `sign_up`, `sign_in`, `mfa`, `password_reset` and `email_change`
    /// sections of `config`.
    pub fn new(
        db: DatabaseConnection,
        mailer: Mailer,
        passwords: Passwords,
        public_url: PublicUrl,
        config: &config::Config,
        mfa_encryption_key: &config::MfaEncryptionKey,
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
            mfa: config.mfa.clone(),
            password_reset: config.password_reset.clone(),
            mfa_cipher: Cipher::new(mfa_encryption_key),
            lifetime: Lifetime::new(&config.sign_in),
            limits: Limits::new(
                &config.sign_up,
                &config.sign_in,
                &config.password_reset,
                &config.email_change,
            ),
            metrics,
        })
    }

    /// The routes of the account endpoints. They need connection information
    /// ([`crate::http::ClientOrigin`]).
    pub fn routes(self: &Arc<Self>) -> Router {
        Router::new()
            .route(SIGN_UP_PATH, post(sign_up::sign_up))
            .route(VERIFY_EMAIL_PATH, post(verify_email::verify_email))
            .route(PASSWORD_RESET_PATH, post(password_reset::request))
            .route(PASSWORD_RESET_CHECK_PATH, post(password_reset::check))
            .route(PASSWORD_RESET_COMPLETE_PATH, post(password_reset::complete))
            .route(SIGN_IN_PATH, post(sign_in::sign_in))
            .route(
                SIGN_IN_SECOND_FACTOR_PATH,
                post(sign_in_second_factor::second_factor),
            )
            .route(REAUTHENTICATE_PATH, post(reauthenticate::reauthenticate))
            .route(ACCOUNT_PATH, get(account::account).patch(account::update))
            .route(ACCOUNT_EMAIL_PATH, post(change_email::request))
            .route(ACCOUNT_EMAIL_CONFIRM_PATH, post(change_email::confirm))
            .route(
                ACCOUNT_PASSWORD_PATH,
                post(change_password::change_password),
            )
            .route(
                SIGN_INS_PATH,
                get(sign_ins::list).delete(sign_ins::revoke_others),
            )
            .route(SIGN_INS_ITEM_PATH, delete(sign_ins::revoke))
            .route(SIGN_OUT_PATH, post(sign_ins::sign_out))
            .route(
                MFA_PATH,
                get(mfa::settings::status).delete(mfa::settings::disable),
            )
            .route(MFA_SETUP_PATH, post(mfa::settings::setup))
            .route(MFA_CONFIRM_PATH, post(mfa::settings::confirm))
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
        Authenticator::new(accounts.db.clone(), &accounts.sign_in)
    }
}
