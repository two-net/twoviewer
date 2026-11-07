//! User accounts (§3.1).
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
//! hides them, and they have no `Display`.

pub mod compromised;
pub mod password;
