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
//! Passwords and their hashes are never logged or audited (MNT-03, AUD-06): their `Debug` output
//! hides them, and they have no `Display`. Neither are the tokens of email links.

pub mod challenge;
pub mod compromised;
pub mod email_link;
pub mod legal;
mod limits;
pub mod password;
mod sign_up;
pub mod user;
mod verify_email;

use std::sync::Arc;

use axum::{Router, routing::post};
use sea_orm::DatabaseConnection;

use self::{limits::Limits, password::Passwords};
use crate::{
    config::{self, PublicUrl},
    email::Mailer,
};

/// The path of sign-up in the API.
pub const SIGN_UP_PATH: &str = "/api/sign-up";
/// The path of email verification in the API.
pub const VERIFY_EMAIL_PATH: &str = "/api/verify-email";

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
    limits: Limits,
}

impl Accounts {
    pub fn new(
        db: DatabaseConnection,
        mailer: Mailer,
        passwords: Passwords,
        public_url: PublicUrl,
        config: &config::Accounts,
        sign_up: &config::SignUp,
    ) -> Arc<Self> {
        Arc::new(Self {
            db,
            mailer,
            passwords,
            public_url,
            config: config.clone(),
            sign_up: sign_up.clone(),
            limits: Limits::new(sign_up),
        })
    }

    /// The routes of the account endpoints. They need connection information
    /// ([`crate::http::ClientOrigin`]).
    pub fn routes(self: &Arc<Self>) -> Router {
        Router::new()
            .route(SIGN_UP_PATH, post(sign_up::sign_up))
            .route(VERIFY_EMAIL_PATH, post(verify_email::verify_email))
            .with_state(self.clone())
    }
}
