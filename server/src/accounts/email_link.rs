//! Single-use links sent by email (EmailLink in §5.2): the ones that verify an address (AUTH-02),
//! reset a password (AUTH-07) and change the address (AUTH-11).
//!
//! A link carries a random [`Token`]. The database stores only its hash (SEC-07,
//! [`super::token`]), so a copy of the database cannot be used to open links. Links are kept for
//! 24 hours after use or expiry, then deleted by the retention job (Table 5.3-A, #66).
//!
//! Links go to the account's address, except those that change it, which go to the new one. So
//! changing the address uses up every link the account has ([`use_up`]): whoever reads the old
//! address's mail must not be able to use them afterwards.

use chrono::Utc;
use sea_orm::{ActiveModelTrait, ConnectionTrait, DbErr, Set, entity::prelude::*};

pub use super::token::{TOKEN_LENGTH as MAX_TOKEN_LENGTH, Token, hash};
use crate::db::EmailAddress;

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
    /// The address a [`Purpose::ChangeEmail`] link changes the account's to, and verifies; `None`
    /// for other links. Lowercase, like the account's (DB-13).
    #[sea_orm(extra = "CHECK (new_email = lower(new_email))")]
    pub new_email: Option<EmailAddress>,
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
    /// Sets a new password for an account whose password was forgotten (AUTH-07).
    ResetPassword,
    /// Changes the address of an account to its `new_email`, which it verifies (AUTH-11).
    ChangeEmail,
}

/// Stores a new link for `purpose` to `user_id`, valid for `lifetime`, and returns its token.
/// `new_email` is the new address of a [`Purpose::ChangeEmail`] link, and `None` for others.
pub async fn create<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    purpose: Purpose,
    new_email: Option<EmailAddress>,
    lifetime: std::time::Duration,
) -> Result<Token, DbErr> {
    debug_assert_eq!(new_email.is_some(), purpose == Purpose::ChangeEmail);
    let token = Token::generate();
    let lifetime = chrono::Duration::from_std(lifetime).unwrap_or(chrono::Duration::MAX);
    ActiveModel {
        user_id: Set(user_id),
        purpose: Set(purpose),
        token_hash: Set(token.hash()),
        expires_at: Set(Utc::now() + lifetime),
        new_email: Set(new_email),
        ..Default::default()
    }
    .insert(db)
    .await?;
    Ok(token)
}

/// Marks `user_id`'s unused links for `purpose` as used, or all of them for `None`, so that none
/// of them works any more.
pub async fn use_up<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    purpose: Option<Purpose>,
) -> Result<(), DbErr> {
    let mut query = Entity::update_many()
        .set(ActiveModel {
            used_at: Set(Some(Utc::now())),
            ..Default::default()
        })
        .filter(Column::UserId.eq(user_id))
        .filter(Column::UsedAt.is_null());
    if let Some(purpose) = purpose {
        query = query.filter(Column::Purpose.eq(purpose));
    }
    query.exec(db).await?;
    Ok(())
}
