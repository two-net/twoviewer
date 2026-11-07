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
//!
//! # Recent authentication (AUTH-10)
//!
//! A handler for a sensitive change takes [`RecentlyAuthenticated`] instead of [`SignedIn`]. It
//! answers `428` with the code `reauthentication_required` when the user last entered their
//! password, at sign-in or by re-authenticating (`POST /api/reauthenticate`), more than
//! `sign_in.reauthenticate_after` ago. The body's `secondFactor` says whether re-authenticating
//! needs a code too, because MFA is on. The web client then asks for the password, re-authenticates
//! and sends the request again.

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

use serde_json::{Map, Value};

use super::{
    mfa,
    sign_in_session::{self, Lifetime},
    token::{self, TOKEN_LENGTH, Token},
};
use crate::{api::ApiError, config};

/// The cookie that holds the token of the browser's sign-in.
pub const COOKIE_NAME: &str = "__Host-twoviewer-session";

/// The answer to a request that needs a sign-in it doesn't have.
pub const NOT_SIGNED_IN: ApiError = ApiError::Unauthorized {
    code: "not_signed_in",
};

/// How often a sign-in's activity is recorded at most.
pub const ACTIVITY_INTERVAL: TimeDelta = TimeDelta::minutes(1);

/// The answer to a request for a sensitive change whose sign-in last authenticated too long ago
/// (AUTH-10).
const REAUTHENTICATION_REQUIRED: &str = "reauthentication_required";

/// The `Set-Cookie` header value that gives the browser `token`, for `max_age`.
pub fn cookie(token: &Token, max_age: Duration) -> HeaderValue {
    token_cookie(COOKIE_NAME, token, max_age)
}

/// The `Set-Cookie` header value that removes the sign-in cookie from the browser.
pub fn clear_cookie() -> HeaderValue {
    set_cookie(COOKIE_NAME, "", 0)
}

/// The `Set-Cookie` header value that gives the browser `token` in the cookie `name`, for
/// `max_age`, with the attributes of the sign-in cookie.
pub(super) fn token_cookie(name: &str, token: &Token, max_age: Duration) -> HeaderValue {
    let mut value = set_cookie(name, token.expose(), max_age.as_secs());
    // Keeps the token out of `Debug` output.
    value.set_sensitive(true);
    value
}

/// The `Set-Cookie` header value that sets the cookie `name` to `value`, for `max_age` seconds.
pub(super) fn set_cookie(name: &str, value: &str, max_age: u64) -> HeaderValue {
    HeaderValue::try_from(format!(
        "{name}={value}; Path=/; Max-Age={max_age}; Secure; HttpOnly; SameSite=Strict"
    ))
    .expect("hexadecimal digits are a valid header")
}

/// The token in the cookie `cookie_name` of `headers`, if there is one of the length of tokens.
pub(super) fn cookie_token<'a>(headers: &'a HeaderMap, cookie_name: &str) -> Option<&'a str> {
    headers
        .get_all(header::COOKIE)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(';'))
        .filter_map(|pair| pair.trim().split_once('='))
        .find_map(|(name, value)| (name == cookie_name).then_some(value))
        .filter(|token| token.len() == TOKEN_LENGTH)
}

/// The signed-in user making a request, and their sign-in.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SignedIn {
    pub user_id: Uuid,
    pub session_id: Uuid,
    /// When the sign-in ends unless it is used again.
    pub expires_at: DateTimeUtc,
    /// When the user last entered their password for this sign-in: when they signed in, or last
    /// re-authenticated (AUTH-10).
    pub authenticated_at: DateTimeUtc,
}

/// A signed-in user who entered their password, and their second factor if MFA is on, within
/// `sign_in.reauthenticate_after` (AUTH-10). Handlers of sensitive changes take it: changing the
/// password, email address or MFA settings, turning on unattended access, and deleting the
/// account.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct RecentlyAuthenticated(pub SignedIn);

/// Finds the sign-in of a request.
#[derive(Clone)]
pub struct Authenticator {
    db: DatabaseConnection,
    lifetime: Lifetime,
    /// How long an authentication counts as recent (AUTH-10).
    reauthenticate_after: TimeDelta,
}

impl Authenticator {
    /// Uses the `sign_in` section of the configuration.
    pub fn new(db: DatabaseConnection, config: &config::SignIn) -> Self {
        Self {
            db,
            lifetime: Lifetime::new(config),
            reauthenticate_after: TimeDelta::from_std(config.reauthenticate_after)
                .unwrap_or(TimeDelta::MAX),
        }
    }

    /// The sign-in whose token is in the cookie of `headers`, if it is active: neither expired
    /// nor revoked. `401` otherwise.
    pub async fn authenticate(&self, headers: &HeaderMap) -> Result<SignedIn, ApiError> {
        self.signed_in(headers).await?.ok_or(NOT_SIGNED_IN)
    }

    /// The sign-in whose token is in the cookie of `headers`, if it is active, after recording
    /// its activity.
    pub async fn signed_in(&self, headers: &HeaderMap) -> Result<Option<SignedIn>, DbErr> {
        let Some(token) = cookie_token(headers, COOKIE_NAME) else {
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
            authenticated_at: session.authenticated_at,
        }))
    }

    /// `signed_in`, if it authenticated recently enough for a sensitive change (AUTH-10).
    /// Otherwise `428` with the code `reauthentication_required`, and `secondFactor` saying
    /// whether re-authenticating needs a code.
    pub async fn recently_authenticated(
        &self,
        signed_in: SignedIn,
    ) -> Result<RecentlyAuthenticated, ApiError> {
        if Utc::now() - signed_in.authenticated_at <= self.reauthenticate_after {
            return Ok(RecentlyAuthenticated(signed_in));
        }
        let second_factor = mfa::factor::Entity::find_by_id(signed_in.user_id)
            .filter(mfa::factor::Column::ConfirmedAt.is_not_null())
            .one(&self.db)
            .await?
            .is_some();
        let mut details = Map::new();
        details.insert("secondFactor".to_owned(), Value::Bool(second_factor));
        Err(ApiError::PreconditionRequired {
            code: REAUTHENTICATION_REQUIRED,
            details,
        })
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

impl<S> FromRequestParts<S> for RecentlyAuthenticated
where
    S: Send + Sync,
    Authenticator: FromRef<S>,
{
    type Rejection = ApiError;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        let authenticator = Authenticator::from_ref(state);
        let signed_in = authenticator.authenticate(&parts.headers).await?;
        authenticator.recently_authenticated(signed_in).await
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
        let token = Token::generate();
        let token = token.expose();
        let token_of = |headers: &HeaderMap| cookie_token(headers, COOKIE_NAME).map(str::to_owned);
        let mut headers = HeaderMap::new();
        assert_eq!(token_of(&headers), None);

        headers.append(header::COOKIE, HeaderValue::from_static("theme=dark"));
        assert_eq!(token_of(&headers), None);
        headers.append(
            header::COOKIE,
            format!("a=1;__Host-twoviewer-session={token}; b=2")
                .parse()
                .unwrap(),
        );
        assert_eq!(token_of(&headers).as_deref(), Some(token));

        let mut other = HeaderMap::new();
        other.insert(
            header::COOKIE,
            format!("twoviewer-session={token}; x__Host-twoviewer-session={token}")
                .parse()
                .unwrap(),
        );
        assert_eq!(token_of(&other), None);
        // Tokens of another length were never issued.
        other.insert(
            header::COOKIE,
            HeaderValue::from_static("__Host-twoviewer-session=abc123"),
        );
        assert_eq!(token_of(&other), None);
    }
}
