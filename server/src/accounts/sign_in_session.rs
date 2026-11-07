//! Browser sign-ins (SignInSession in §5.2).
//!
//! Signing in creates one, and the browser holds its [`Token`](super::token::Token) in a cookie
//! ([`super::signed_in`]). The database stores only the token's hash (SEC-07), so a copy of the
//! database cannot be used to sign in.
//!
//! A sign-in ends at `expires_at`, `accounts.sign_in.session_lifetime` after it started (AUTH-09).
//! Ending sign-ins after inactivity, and listing and revoking them, come with #26. They are kept
//! for 30 days after expiry or revocation, then deleted by the retention job (Table 5.3-A, #66).

use sea_orm::entity::prelude::*;

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
    pub last_active_at: DateTimeUtc,
    pub expires_at: DateTimeUtc,
    /// Where the user signed in from, which they will see in their list of sign-ins (AUTH-09).
    pub ip_address: IpNetwork,
    /// Browser and operating system, such as "Firefox on Linux".
    pub client_description: String,
    /// When the sign-in was revoked; `None` while it isn't.
    pub revoked_at: Option<DateTimeUtc>,
    #[sea_orm(belongs_to, from = "user_id", to = "id", on_delete = "Cascade")]
    pub user: BelongsTo<super::user::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}
