//! Multi-factor authentication with an authenticator app (AUTH-05, AUTH-06).
//!
//! A user turns MFA on in two steps ([`settings`]): setting up an app stores a new secret
//! ([`totp`]), encrypted ([`cipher`], SEC-07), and shows it as a QR code ([`qr`]); a code from the
//! app then confirms it, which turns MFA on and issues 10 recovery codes ([`recovery_code`]),
//! shown once and stored only as hashes. Turning MFA off deletes the secret and the codes. Both
//! need a recent authentication (AUTH-10), and are audited (`mfa_enabled`, `mfa_disabled`).
//!
//! With MFA on, signing in takes a second step (AUTH-05), and so does re-authenticating (AUTH-10):
//! a code from the app, or an unused recovery code. [`second_factor`] checks them, for both, and
//! throttles wrong ones per user (SEC-08).

pub mod cipher;
pub mod factor;
pub mod qr;
pub mod recovery_code;
pub mod second_factor;
pub(super) mod settings;
pub mod totp;

use sea_orm::{ConnectionTrait, prelude::Uuid};
use serde_json::json;
use twoviewer_protocol::Event;

use crate::realtime::{self, Topic};

/// The event that tells a user's browsers that MFA was turned on or off, so that open settings
/// reload.
pub const CHANGED_EVENT: &str = "mfa.changed";

/// Tells `user_id`'s browsers that MFA was turned on or off, once the transaction of `db` commits.
async fn changed<C: ConnectionTrait>(db: &C, user_id: Uuid) -> Result<(), realtime::PublishError> {
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
