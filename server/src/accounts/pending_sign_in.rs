//! Sign-ins waiting for their second step (AUTH-05).
//!
//! When the password of a user with MFA is right, sign-in doesn't create a sign-in yet. It stores
//! a pending one, and gives the browser its [`Token`] in the cookie [`COOKIE_NAME`], with the same
//! attributes as the sign-in cookie, so page scripts cannot read it either (SEC-09). The second
//! step (`POST /api/sign-in/second-factor`) finds it by that cookie, and only a right code turns it
//! into a sign-in. The database stores only the token's hash (SEC-07).
//!
//! A pending sign-in lasts `mfa.sign_in_step_timeout`. It is deleted when the second step
//! completes, and expired ones are deleted in small batches whenever a new one is stored, so the
//! table holds little and nothing for long.

use std::time::Duration;

use axum::http::{HeaderMap, HeaderValue};
use chrono::Utc;
use sea_orm::{
    ActiveModelTrait, ConnectionTrait, DbErr, QueryFilter, Set,
    entity::prelude::*,
    sea_query::{LockBehavior, LockType, Query},
};

use super::{
    signed_in,
    token::{self, Token},
};

#[sea_orm::model]
#[derive(Clone, Debug, PartialEq, Eq, DeriveEntityModel)]
#[sea_orm(table_name = "pending_sign_in")]
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
    /// When the second step stops waiting for a code.
    #[sea_orm(indexed)]
    pub expires_at: DateTimeUtc,
    #[sea_orm(belongs_to, from = "user_id", to = "id", on_delete = "Cascade")]
    pub user: BelongsTo<super::user::Entity>,
}

impl ActiveModelBehavior for ActiveModel {}

/// The cookie that holds the token of the browser's pending sign-in.
pub const COOKIE_NAME: &str = "__Host-twoviewer-sign-in";

/// At most this many expired pending sign-ins are deleted each time one is stored. Each sign-in
/// stores at most one, so this keeps up.
const CLEANUP_BATCH: u64 = 100;

/// Stores a pending sign-in of `user_id` for `lifetime`, and returns its token.
pub async fn create<C: ConnectionTrait>(
    db: &C,
    user_id: Uuid,
    lifetime: Duration,
) -> Result<Token, DbErr> {
    let now = Utc::now();
    remove_expired(db, now).await?;
    let token = Token::generate();
    let lifetime = chrono::Duration::from_std(lifetime).unwrap_or(chrono::Duration::MAX);
    ActiveModel {
        user_id: Set(user_id),
        token_hash: Set(token.hash()),
        created_at: Set(now),
        expires_at: Set(now
            .checked_add_signed(lifetime)
            .unwrap_or(DateTimeUtc::MAX_UTC)),
        ..Default::default()
    }
    .insert(db)
    .await?;
    Ok(token)
}

/// The pending sign-in whose token is in the cookie of `headers`, if it hasn't expired.
pub async fn find<C: ConnectionTrait>(db: &C, headers: &HeaderMap) -> Result<Option<Model>, DbErr> {
    let Some(token) = signed_in::cookie_token(headers, COOKIE_NAME) else {
        return Ok(None);
    };
    Entity::find()
        .filter(Column::TokenHash.eq(token::hash(token)))
        .filter(Column::ExpiresAt.gt(Utc::now()))
        .one(db)
        .await
}

/// Deletes the pending sign-in `id`, once its second step has completed, and returns whether it
/// was still there: a concurrent second step may have completed it first.
pub async fn complete<C: ConnectionTrait>(db: &C, id: Uuid) -> Result<bool, DbErr> {
    let deleted = Entity::delete_many()
        .filter(Column::Id.eq(id))
        .filter(Column::ExpiresAt.gt(Utc::now()))
        .exec(db)
        .await?;
    Ok(deleted.rows_affected == 1)
}

/// The `Set-Cookie` header value that gives the browser the token of its pending sign-in.
pub fn cookie(token: &Token, max_age: Duration) -> HeaderValue {
    signed_in::token_cookie(COOKIE_NAME, token, max_age)
}

/// The `Set-Cookie` header value that removes the cookie of the pending sign-in.
pub fn clear_cookie() -> HeaderValue {
    signed_in::set_cookie(COOKIE_NAME, "", 0)
}

/// Deletes some expired pending sign-ins. Rows that another transaction has locked are skipped
/// rather than waited for, as in [`crate::throttle`].
async fn remove_expired<C: ConnectionTrait>(db: &C, now: DateTimeUtc) -> Result<(), DbErr> {
    let expired = Query::select()
        .column(Column::Id)
        .from(Entity)
        .and_where(Column::ExpiresAt.lte(now))
        .limit(CLEANUP_BATCH)
        .lock_with_behavior(LockType::Update, LockBehavior::SkipLocked)
        .to_owned();
    Entity::delete_many()
        .filter(Column::Id.in_subquery(expired))
        .exec(db)
        .await?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cookie_is_like_the_sign_in_cookie() {
        let token = Token::generate();

        assert_eq!(
            cookie(&token, Duration::from_secs(300)).to_str().unwrap(),
            format!(
                "__Host-twoviewer-sign-in={}; Path=/; Max-Age=300; Secure; HttpOnly; \
                 SameSite=Strict",
                token.expose()
            )
        );
        assert_eq!(
            clear_cookie().to_str().unwrap(),
            "__Host-twoviewer-sign-in=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Strict"
        );
    }
}
