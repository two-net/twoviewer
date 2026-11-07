//! Single-use links sent by email (EmailLink in §5.2), such as the one that verifies an address
//! (AUTH-02).
//!
//! A link carries a random [`Token`]. The database stores only its hash (SEC-07,
//! [`super::token`]), so a copy of the database cannot be used to open links. Links are kept for
//! 24 hours after use or expiry, then deleted by the retention job (Table 5.3-A, #66).

use chrono::Utc;
use sea_orm::{ActiveModelTrait, ConnectionTrait, DbErr, Set, entity::prelude::*};

pub use super::token::{TOKEN_LENGTH as MAX_TOKEN_LENGTH, Token, hash};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "email_link")]
pub struct Model {
    #[sea_orm(
        primary_key,
        auto_increment = false,
        default_expr = "Expr::cust(\"gen_random_uuid()\")"
    )]
    pub id: Uuid,
    #[sea_orm(indexed)]
    pub user_id: Uuid,
    pub purpose: Purpose,
    /// SHA-256 of the token, never the token itself (SEC-07).
    #[sea_orm(unique)]
    pub token_hash: Vec<u8>,
    pub expires_at: DateTimeUtc,
    /// When the link was used; `None` until then.
    pub used_at: Option<DateTimeUtc>,
    #[sea_orm(default_expr = "Expr::current_timestamp()")]
    pub created_at: DateTimeUtc,
    #[sea_orm(belongs_to, from = "user_id", to = "id", on_delete = "Cascade")]
    pub user: BelongsTo<super::user::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}

/// What a link does. Stored as text: schema sync cannot add values to a PostgreSQL enum.
#[derive(Clone, Copy, Debug, PartialEq, Eq, EnumIter, DeriveActiveEnum)]
#[sea_orm(
    rs_type = "String",
    db_type = "String(StringLen::None)",
    rename_all = "snake_case"
)]
pub enum Purpose {
    /// Verifies the email address of a new account (AUTH-02).
    VerifyEmail,
}

/// Stores a new link for `purpose` to `user_id`, valid for `lifetime`, and returns its token.
pub async fn create<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    purpose: Purpose,
    lifetime: std::time::Duration,
) -> Result<Token, DbErr> {
    let token = Token::generate();
    let lifetime = chrono::Duration::from_std(lifetime).unwrap_or(chrono::Duration::MAX);
    ActiveModel {
        user_id: Set(user_id),
        purpose: Set(purpose),
        token_hash: Set(token.hash()),
        expires_at: Set(Utc::now() + lifetime),
        ..Default::default()
    }
    .insert(db)
    .await?;
    Ok(token)
}
