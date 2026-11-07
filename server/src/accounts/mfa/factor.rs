//! A user's authenticator app (MfaFactor in §5.2): at most one per user, keyed by the user.
//!
//! Setting one up stores its secret, encrypted ([`super::cipher`]), with `confirmed_at` unset. MFA
//! is on only once the user has confirmed it with a code from the app, which also issues the
//! recovery codes ([`super::recovery_code`]). Turning MFA off deletes the row, and the recovery
//! codes with it.

use sea_orm::entity::prelude::*;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "mfa_factor")]
pub struct Model {
    /// The user's ID: one factor per user.
    #[sea_orm(primary_key, auto_increment = false)]
    pub user_id: Uuid,
    /// The secret of the authenticator app, encrypted for this user ([`super::cipher`]), never
    /// the secret itself (SEC-07).
    pub secret_encrypted: Vec<u8>,
    /// When the user confirmed the app with a code, which turned MFA on; `None` while it is being
    /// set up.
    pub confirmed_at: Option<DateTimeUtc>,
    /// The step of the last code accepted, so that no code is accepted twice (RFC 6238 §5.2).
    pub last_used_step: Option<i64>,
    #[sea_orm(default_expr = "Expr::current_timestamp()")]
    pub created_at: DateTimeUtc,
    #[sea_orm(belongs_to, from = "user_id", to = "id", on_delete = "Cascade")]
    pub user: BelongsTo<crate::accounts::user::Entity>,
    #[sea_orm(has_many)]
    pub recovery_codes: HasMany<super::recovery_code::Entity>,
}

impl Model {
    /// Whether MFA is on: the app has been confirmed.
    pub fn is_enabled(&self) -> bool {
        self.confirmed_at.is_some()
    }
}

impl ActiveModelBehavior for ActiveModel {}
