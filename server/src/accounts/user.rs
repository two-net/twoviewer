//! Registered accounts (User in §5.2).
//!
//! The table is `user_account` rather than `user`, which PostgreSQL reserves: in hand-written SQL,
//! `SELECT * FROM user` would quietly return the current database user instead.

use sea_orm::entity::prelude::*;

use super::password::PasswordHash;
use crate::db::EmailAddress;

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "user_account")]
pub struct Model {
    #[sea_orm(
        primary_key,
        auto_increment = false,
        default_expr = "Expr::cust(\"gen_random_uuid()\")"
    )]
    pub id: Uuid,
    #[sea_orm(unique, extra = "CHECK (email = lower(email))")]
    pub email: EmailAddress,
    /// When the address was verified (AUTH-02); `None` until then.
    pub email_verified_at: Option<DateTimeUtc>,
    pub password_hash: PasswordHash,
    pub display_name: String,
    /// The versions of the Terms of Service and Privacy Notice the user accepted (AUTH-01), as in
    /// [`super::legal`].
    pub accepted_terms_version: String,
    pub accepted_privacy_version: String,
    pub terms_accepted_at: DateTimeUtc,
    #[sea_orm(default_expr = "Expr::current_timestamp()")]
    pub created_at: DateTimeUtc,
    #[sea_orm(has_many)]
    pub email_links: HasMany<super::email_link::Entity>,
    #[sea_orm(has_many)]
    pub sign_in_sessions: HasMany<super::sign_in_session::Entity>,
}

impl Model {
    /// Whether the email address is verified. Until it is, the user can sign in but cannot pair
    /// devices or start sessions (AUTH-02).
    pub fn is_verified(&self) -> bool {
        self.email_verified_at.is_some()
    }
}

impl ActiveModelBehavior for ActiveModel {}
