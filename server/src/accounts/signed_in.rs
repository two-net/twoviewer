//! Who is making a request: the sign-in whose token the request's cookie holds.
//!
//! Signing in ([`super::sign_in`]) sets the cookie [`COOKIE_NAME`] to the token of a new
//! [`sign_in_session`]. The cookie is:
//!
//! - `HttpOnly`, so that page scripts cannot read it (SEC-09);
//! - `Secure` with the `__Host-` prefix, so that browsers send it over HTTPS only, and only this
//!   host, not a subdomain, can set or replace it;
//! - `SameSite=Strict`, so that browsers don't send it with requests that other sites start.
//!   Together with the API taking only JSON bodies ([`crate::api`]), other sites cannot make a
//!   user's browser act in their name. #26 completes the protection against forged requests;
//! - kept for `sign_in.session_lifetime`, the longest a sign-in lasts (AUTH-09).
//!
//! A handler that needs a signed-in user takes [`SignedIn`], which answers `401` with the code
//! `not_signed_in` to requests without an active sign-in. Its router's state must provide an
//! [`Authenticator`], with `impl FromRef<State> for Authenticator`.

use std::time::Duration;

use axum::{
    extract::{FromRef, FromRequestParts},
    http::{HeaderMap, HeaderValue, header, request::Parts},
};
use chrono::Utc;
use sea_orm::{ColumnTrait, DatabaseConnection, EntityTrait, QueryFilter, prelude::Uuid};

use super::{
    sign_in_session,
    token::{self, TOKEN_LENGTH, Token},
};
use crate::api::ApiError;

/// The cookie that holds the token of the browser's sign-in.
pub const COOKIE_NAME: &str = "__Host-twoviewer-session";

/// The answer to a request that needs a sign-in it doesn't have.
pub const NOT_SIGNED_IN: ApiError = ApiError::Unauthorized {
    code: "not_signed_in",
};

/// The `Set-Cookie` header value that gives the browser `token`, for `max_age`.
pub fn cookie(token: &Token, max_age: Duration) -> HeaderValue {
    let mut value = HeaderValue::try_from(format!(
        "{COOKIE_NAME}={}; Path=/; Max-Age={}; Secure; HttpOnly; SameSite=Strict",
        token.expose(),
        max_age.as_secs()
    ))
    .expect("hexadecimal digits are a valid header");
    // Keeps the token out of `Debug` output.
    value.set_sensitive(true);
    value
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
}

/// Finds the sign-in of a request.
#[derive(Clone)]
pub struct Authenticator {
    db: DatabaseConnection,
}

impl Authenticator {
    pub fn new(db: DatabaseConnection) -> Self {
        Self { db }
    }

    /// The sign-in whose token is in the cookie of `headers`, if it is active: neither expired
    /// nor revoked.
    pub async fn authenticate(&self, headers: &HeaderMap) -> Result<SignedIn, ApiError> {
        let Some(token) = token(headers).filter(|token| token.len() == TOKEN_LENGTH) else {
            return Err(NOT_SIGNED_IN);
        };
        let session = sign_in_session::Entity::find()
            .filter(sign_in_session::Column::TokenHash.eq(token::hash(token)))
            .filter(sign_in_session::Column::RevokedAt.is_null())
            .filter(sign_in_session::Column::ExpiresAt.gt(Utc::now()))
            .one(&self.db)
            .await?;
        session
            .map(|session| SignedIn {
                user_id: session.user_id,
                session_id: session.id,
            })
            .ok_or(NOT_SIGNED_IN)
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
