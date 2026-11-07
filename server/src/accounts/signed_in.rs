//! Who is making a request: the sign-in whose token the request's cookie holds.
//!
//! Signing in ([`super::sign_in`]) sets the cookie [`COOKIE_NAME`] to the token of a new
//! [`sign_in_session`]. The cookie is:
//!
//! - `HttpOnly`, so that page scripts cannot read it (SEC-09);
//! - `Secure` with the `__Host-` prefix, so that browsers send it over HTTPS only, and only this
//!   host, not a subdomain, can set or replace it;
//! - `SameSite=Strict`, so that browsers don't send it with requests that other sites start.
//!   Sites that count as the same site, such as other subdomains, are kept out by the origin check
//!   on every request that changes something ([`crate::http`]);
//! - kept for `sign_in.session_lifetime`, the longest a sign-in lasts (AUTH-09). The server ends
//!   it earlier when it isn't used ([`sign_in_session::Lifetime`]).
//!
//! A handler that needs a signed-in user takes [`SignedIn`], which answers `401` with the code
//! `not_signed_in` to requests without an active sign-in. Its router's state must provide an
//! [`Authenticator`], with `impl FromRef<State> for Authenticator`.
//!
//! Every request that a sign-in authenticates counts as activity: it moves the sign-in's
//! `last_active_at` to now, and its `expires_at` on. To spare the database a write per request,
//! that happens at most once per [`ACTIVITY_INTERVAL`], so a sign-in may end that much early.

use std::time::Duration;

use axum::{
    extract::{FromRef, FromRequestParts},
    http::{HeaderMap, HeaderValue, header, request::Parts},
};
use chrono::{TimeDelta, Utc};
use sea_orm::{
    ColumnTrait, DatabaseConnection, DbErr, EntityTrait, QueryFilter, Set,
    prelude::{DateTimeUtc, Uuid},
};

use super::{
    sign_in_session::{self, Lifetime},
    token::{self, TOKEN_LENGTH, Token},
};
use crate::api::ApiError;

/// The cookie that holds the token of the browser's sign-in.
pub const COOKIE_NAME: &str = "__Host-twoviewer-session";

/// The answer to a request that needs a sign-in it doesn't have.
pub const NOT_SIGNED_IN: ApiError = ApiError::Unauthorized {
    code: "not_signed_in",
};

/// How often a sign-in's activity is recorded at most.
pub const ACTIVITY_INTERVAL: TimeDelta = TimeDelta::minutes(1);

/// The `Set-Cookie` header value that gives the browser `token`, for `max_age`.
pub fn cookie(token: &Token, max_age: Duration) -> HeaderValue {
    let mut value = set_cookie(token.expose(), max_age.as_secs());
    // Keeps the token out of `Debug` output.
    value.set_sensitive(true);
    value
}

/// The `Set-Cookie` header value that removes the sign-in cookie from the browser.
pub fn clear_cookie() -> HeaderValue {
    set_cookie("", 0)
}

fn set_cookie(value: &str, max_age: u64) -> HeaderValue {
    HeaderValue::try_from(format!(
        "{COOKIE_NAME}={value}; Path=/; Max-Age={max_age}; Secure; HttpOnly; SameSite=Strict"
    ))
    .expect("hexadecimal digits are a valid header")
}

/// The token in the sign-in cookie of `headers`, if there is one.
fn token(headers: &HeaderMap) -> Option<&str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find_map(|(name, value)| (name == COOKIE_NAME).then_some(value))
}

/// The signed-in user making a request, and their sign-in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignedIn {
    pub user_id: Uuid,
    pub session_id: Uuid,
    /// When the sign-in ends unless it is used again.
    pub expires_at: DateTimeUtc,
}

/// Finds the sign-in of a request.
#[derive(Clone)]
pub struct Authenticator {
    db: DatabaseConnection,
    lifetime: Lifetime,
}

impl Authenticator {
    pub fn new(db: DatabaseConnection, lifetime: Lifetime) -> Self {
        Self { db, lifetime }
    }

    /// The sign-in whose token is in the cookie of `headers`, if it is active: neither expired
    /// nor revoked. `401` otherwise.
    pub async fn authenticate(&self, headers: &HeaderMap) -> Result<SignedIn, ApiError> {
        self.signed_in(headers).await?.ok_or(NOT_SIGNED_IN)
    }

    /// The sign-in whose token is in the cookie of `headers`, if it is active, after recording
    /// its activity.
    pub async fn signed_in(&self, headers: &HeaderMap) -> Result<Option<SignedIn>, DbErr> {
        let Some(token) = token(headers).filter(|token| token.len() == TOKEN_LENGTH) else {
            return Ok(None);
        };
        let now = Utc::now();
        let Some(session) = sign_in_session::Entity::find()
            .filter(sign_in_session::Column::TokenHash.eq(token::hash(token)))
            .filter(sign_in_session::Column::RevokedAt.is_null())
            .filter(sign_in_session::Column::ExpiresAt.gt(now))
            .one(&self.db)
            .await?
        else {
            return Ok(None);
        };

        let mut expires_at = session.expires_at;
        if now - session.last_active_at >= ACTIVITY_INTERVAL {
            expires_at = self.lifetime.expiry(session.created_at, now);
            // Not if it was revoked in the meantime, so that it stays as it was revoked.
            sign_in_session::Entity::update_many()
                .set(sign_in_session::ActiveModel {
                    last_active_at: Set(now),
                    expires_at: Set(expires_at),
                    ..Default::default()
                })
                .filter(sign_in_session::Column::Id.eq(session.id))
                .filter(sign_in_session::Column::RevokedAt.is_null())
                .exec(&self.db)
                .await?;
        }
        Ok(Some(SignedIn {
            user_id: session.user_id,
            session_id: session.id,
            expires_at,
        }))
    }

    /// When the sign-in `session_id` ends unless it is used again, if it is active. Its activity
    /// isn't recorded.
    pub async fn active_until(&self, session_id: Uuid) -> Result<Option<DateTimeUtc>, DbErr> {
        let session = sign_in_session::Entity::find_by_id(session_id)
            .filter(sign_in_session::Column::RevokedAt.is_null())
            .filter(sign_in_session::Column::ExpiresAt.gt(Utc::now()))
            .one(&self.db)
            .await?;
        Ok(session.map(|session| session.expires_at))
    }
}

impl<S> FromRequestParts<S> for SignedIn
where
    S: Send + Sync,
    Authenticator: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        Authenticator::from_ref(state)
            .authenticate(&parts.headers)
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_cookie_is_for_https_only_and_hidden_from_scripts_and_other_sites() {
        let token = Token::generate();

        let value = cookie(&token, Duration::from_secs(30 * 24 * 60 * 60));

        assert_eq!(
            value.to_str().unwrap(),
            format!(
                "__Host-twoviewer-session={}; Path=/; Max-Age=2592000; Secure; HttpOnly; \
                 SameSite=Strict",
                token.expose()
            )
        );
        assert!(!format!("{value:?}").contains(token.expose()));
    }

    #[test]
    fn clearing_the_cookie_replaces_it_with_one_that_expires_at_once() {
        // The same name and attributes, or browsers would keep the cookie and add another.
        assert_eq!(
            clear_cookie().to_str().unwrap(),
            "__Host-twoviewer-session=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Strict"
        );
    }

    #[test]
    fn the_token_is_found_among_other_cookies() {
        let mut headers = HeaderMap::new();
        assert_eq!(token(&headers), None);

        headers.append(header::COOKIE, HeaderValue::from_static("theme=dark"));
        assert_eq!(token(&headers), None);
        headers.append(
            header::COOKIE,
            HeaderValue::from_static("a=1;__Host-twoviewer-session=abc123; b=2"),
        );
        assert_eq!(token(&headers), Some("abc123"));

        let mut other = HeaderMap::new();
        other.insert(
            header::COOKIE,
            HeaderValue::from_static("twoviewer-session=abc; x__Host-twoviewer-session=def"),
        );
        assert_eq!(token(&other), None);
    }
}
