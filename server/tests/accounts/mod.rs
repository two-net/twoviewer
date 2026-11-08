//! The account endpoints in-process, against a real PostgreSQL and Mailpit, with each request's
//! client address chosen by the test. Shared by the account tests.
#![allow(dead_code, reason = "each test uses only some of the helpers")]

use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{HeaderMap, Method, Request, StatusCode, header},
};
use chrono::{TimeDelta, Utc};
use sea_orm::{
    ActiveEnum, ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend,
    EntityTrait, QueryFilter, Set, Statement, prelude::Uuid,
};
use serde_json::{Value, json};
use testcontainers_modules::{postgres::Postgres, testcontainers::ContainerAsync};
use tower::ServiceExt;
use twoviewer_server::{
    accounts::{
        Accounts, legal,
        mfa::{
            factor,
            totp::{self, Secret},
        },
        password::{Password, Passwords},
        sign_in_session, user,
    },
    audit::Action,
    config::{self, PublicUrl},
    db::{self, EmailAddress, migrate},
    email::Mailer,
    http,
    metrics::Metrics,
};

use crate::common::{
    LOCK_TIMEOUT,
    mailpit::{Mailpit, mailpit, wait_until},
    mfa_key, postgres,
};

pub const PASSWORD: &str = "violet otter harbor 92";
pub const FIREFOX: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:133.0) Gecko/20100101 Firefox/133.0";
pub const SAFARI: &str = "Mozilla/5.0 (iPhone; CPU iPhone OS 18_1 like Mac OS X) \
                          AppleWebKit/605.1.15 (KHTML, like Gecko) Version/18.1 Mobile/15E148 \
                          Safari/604.1";
/// The host of the server, as browsers send it.
pub const HOST: &str = "twoviewer.test";

pub fn client() -> IpAddr {
    "203.0.113.7".parse().unwrap()
}

pub fn elsewhere() -> IpAddr {
    "198.51.100.20".parse().unwrap()
}

/// Settings that hash in well under a millisecond: these tests aren't about the cost.
pub fn cheap_hashes(config: &mut config::Config) {
    config.passwords.hash_memory_kib = 64;
    config.passwords.hash_iterations = 1;
}

/// The account endpoints, with the database and Mailpit they use.
pub struct App {
    router: Router,
    pub db: DatabaseConnection,
    pub mailpit: Mailpit,
    pub metrics: Arc<Metrics>,
    /// Hashes passwords with the same settings as the server.
    passwords: Passwords,
    _postgres: ContainerAsync<Postgres>,
}

/// The answer to a request: its status, headers and body.
#[derive(Debug)]
pub struct Answer {
    pub status: StatusCode,
    pub headers: HeaderMap,
    pub body: String,
}

impl Answer {
    pub fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap()
    }

    /// The `Set-Cookie` header of the answer, if it has one.
    pub fn set_cookie(&self) -> Option<&str> {
        self.headers
            .get(header::SET_COOKIE)
            .map(|value| value.to_str().unwrap())
    }

    /// The `name=value` of the cookie the answer sets.
    pub fn cookie(&self) -> String {
        let set_cookie = self.set_cookie().unwrap();
        set_cookie.split(';').next().unwrap().to_owned()
    }

    pub fn retry_after(&self) -> u64 {
        self.headers[header::RETRY_AFTER]
            .to_str()
            .unwrap()
            .parse()
            .unwrap()
    }
}

pub async fn app(configure: impl FnOnce(&mut config::Config)) -> App {
    let mut config = config::Config::default();
    cheap_hashes(&mut config);
    configure(&mut config);

    let ((postgres, url), mailpit) = tokio::join!(postgres(), mailpit());
    migrate::migrate(&url, LOCK_TIMEOUT).await.unwrap();
    let db = db::connect(&url).await.unwrap();
    let metrics = Arc::new(Metrics::new());
    let mailer = Mailer::new(&mailpit.config(), None, metrics.clone()).unwrap();
    let passwords = Passwords::new(&config.passwords).unwrap();
    let accounts = Accounts::new(
        db.clone(),
        mailer,
        passwords.clone(),
        PublicUrl::parse("https://twoviewer.test").unwrap(),
        &config,
        &mfa_key(),
        metrics.clone(),
    );
    App {
        router: http::router(accounts.routes(), metrics.clone(), Duration::ZERO),
        db,
        mailpit,
        metrics,
        passwords,
        _postgres: postgres,
    }
}

impl App {
    pub async fn send(&self, mut request: Request<Body>, from: IpAddr) -> Answer {
        request
            .extensions_mut()
            .insert(ConnectInfo(SocketAddr::new(from, 50_000)));
        let response = self.router.clone().oneshot(request).await.unwrap();
        let status = response.status();
        let headers = response.headers().clone();
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        Answer {
            status,
            headers,
            body: String::from_utf8(bytes.to_vec()).unwrap(),
        }
    }

    /// A request without a body from Firefox, with `cookie` and `headers`.
    pub async fn request(
        &self,
        method: Method,
        uri: &str,
        cookie: Option<&str>,
        headers: &[(&str, &str)],
    ) -> Answer {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::USER_AGENT, FIREFOX);
        if let Some(cookie) = cookie {
            request = request.header(header::COOKIE, cookie);
        }
        for (name, value) in headers {
            request = request.header(*name, *value);
        }
        self.send(request.body(Body::empty()).unwrap(), client())
            .await
    }

    pub async fn sign_in_with(
        &self,
        email: &str,
        password: &str,
        from: IpAddr,
        user_agent: &str,
    ) -> Answer {
        let body = json!({ "email": email, "password": password });
        let request = Request::post("/api/sign-in")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::USER_AGENT, user_agent)
            .body(Body::from(body.to_string()))
            .unwrap();
        self.send(request, from).await
    }

    pub async fn sign_in_from(&self, email: &str, password: &str, from: IpAddr) -> Answer {
        self.sign_in_with(email, password, from, FIREFOX).await
    }

    pub async fn sign_in(&self, email: &str, password: &str) -> Answer {
        self.sign_in_from(email, password, client()).await
    }

    /// The cookie of a new sign-in of `email`, with [`PASSWORD`].
    pub async fn signed_in(&self, email: &str) -> String {
        let answer = self.sign_in(email, PASSWORD).await;
        assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.body);
        answer.cookie()
    }

    /// `GET /api/account`, with `cookie` if there is one.
    pub async fn account(&self, cookie: Option<&str>) -> Answer {
        self.request(Method::GET, "/api/account", cookie, &[]).await
    }

    /// Whether `cookie` is that of an active sign-in.
    pub async fn is_signed_in(&self, cookie: &str) -> bool {
        match self.account(Some(cookie)).await.status {
            StatusCode::OK => true,
            StatusCode::UNAUTHORIZED => false,
            status => panic!("unexpected {status}"),
        }
    }

    /// `GET /api/sign-ins`.
    pub async fn sign_ins(&self, cookie: Option<&str>) -> Answer {
        self.request(Method::GET, "/api/sign-ins", cookie, &[])
            .await
    }

    /// `DELETE /api/sign-ins/{id}`.
    pub async fn revoke(&self, cookie: &str, id: impl std::fmt::Display) -> Answer {
        self.request(
            Method::DELETE,
            &format!("/api/sign-ins/{id}"),
            Some(cookie),
            &[],
        )
        .await
    }

    /// `DELETE /api/sign-ins`.
    pub async fn revoke_others(&self, cookie: &str) -> Answer {
        self.request(Method::DELETE, "/api/sign-ins", Some(cookie), &[])
            .await
    }

    /// `POST /api/sign-out`, with `cookie` if there is one.
    pub async fn sign_out(&self, cookie: Option<&str>) -> Answer {
        self.request(Method::POST, "/api/sign-out", cookie, &[])
            .await
    }

    /// A request with `body` as JSON from Firefox, with `cookie` if there is one.
    pub async fn send_json(
        &self,
        method: Method,
        uri: &str,
        cookie: Option<&str>,
        body: Value,
    ) -> Answer {
        let mut request = Request::builder()
            .method(method)
            .uri(uri)
            .header(header::USER_AGENT, FIREFOX)
            .header(header::CONTENT_TYPE, "application/json");
        if let Some(cookie) = cookie {
            request = request.header(header::COOKIE, cookie);
        }
        self.send(
            request.body(Body::from(body.to_string())).unwrap(),
            client(),
        )
        .await
    }

    /// `POST /api/sign-in/second-factor` with `code`, and the pending sign-in's `cookie`.
    pub async fn second_factor(&self, cookie: Option<&str>, code: &str) -> Answer {
        self.send_json(
            Method::POST,
            "/api/sign-in/second-factor",
            cookie,
            json!({ "code": code }),
        )
        .await
    }

    /// `POST /api/reauthenticate` with `password`, and `code` if there is one.
    pub async fn reauthenticate(&self, cookie: &str, password: &str, code: Option<&str>) -> Answer {
        let mut body = json!({ "password": password });
        if let Some(code) = code {
            body["code"] = json!(code);
        }
        self.send_json(Method::POST, "/api/reauthenticate", Some(cookie), body)
            .await
    }

    /// `PATCH /api/account` with `display_name`.
    pub async fn change_display_name(&self, cookie: Option<&str>, display_name: &str) -> Answer {
        self.send_json(
            Method::PATCH,
            "/api/account",
            cookie,
            json!({ "displayName": display_name }),
        )
        .await
    }

    /// `POST /api/account/email` with `email`.
    pub async fn change_email(&self, cookie: &str, email: &str) -> Answer {
        self.send_json(
            Method::POST,
            "/api/account/email",
            Some(cookie),
            json!({ "email": email }),
        )
        .await
    }

    /// `POST /api/account/email/confirm` with `token`, without a sign-in.
    pub async fn confirm_email(&self, token: &str) -> Answer {
        self.send_json(
            Method::POST,
            "/api/account/email/confirm",
            None,
            json!({ "token": token }),
        )
        .await
    }

    /// `POST /api/account/password` with the `current` and the `new` password.
    pub async fn change_password(&self, cookie: &str, current: &str, new: &str) -> Answer {
        self.send_json(
            Method::POST,
            "/api/account/password",
            Some(cookie),
            json!({ "currentPassword": current, "newPassword": new }),
        )
        .await
    }

    /// `GET /api/mfa`.
    pub async fn mfa(&self, cookie: &str) -> Answer {
        self.request(Method::GET, "/api/mfa", Some(cookie), &[])
            .await
    }

    /// `POST /api/mfa/setup`.
    pub async fn mfa_setup(&self, cookie: &str) -> Answer {
        self.request(Method::POST, "/api/mfa/setup", Some(cookie), &[])
            .await
    }

    /// `POST /api/mfa/confirm` with `code`.
    pub async fn mfa_confirm(&self, cookie: &str, code: &str) -> Answer {
        self.send_json(
            Method::POST,
            "/api/mfa/confirm",
            Some(cookie),
            json!({ "code": code }),
        )
        .await
    }

    /// `DELETE /api/mfa`.
    pub async fn mfa_disable(&self, cookie: &str) -> Answer {
        self.request(Method::DELETE, "/api/mfa", Some(cookie), &[])
            .await
    }

    /// Turns MFA on for the user signed in with `cookie`, which must have authenticated recently,
    /// and returns the app's secret and the recovery codes.
    pub async fn enable_mfa(&self, cookie: &str) -> (Secret, Vec<String>) {
        let setup = self.mfa_setup(cookie).await;
        assert_eq!(setup.status, StatusCode::OK, "{}", setup.body);
        let secret = secret_of(&setup);
        let confirmed = self
            .mfa_confirm(cookie, &code_at(&secret, totp::step(Utc::now())))
            .await;
        assert_eq!(confirmed.status, StatusCode::OK, "{}", confirmed.body);
        let codes = confirmed.json()["recoveryCodes"]
            .as_array()
            .unwrap()
            .iter()
            .map(|code| code.as_str().unwrap().to_owned())
            .collect();
        (secret, codes)
    }

    /// The factor of `user_id`, if there is one.
    pub async fn factor(&self, user_id: Uuid) -> Option<factor::Model> {
        factor::Entity::find_by_id(user_id)
            .one(&self.db)
            .await
            .unwrap()
    }

    /// A code of the app with `secret` of `user_id` that the server would accept now: of the
    /// earliest step within the clock skew that is later than the last one used.
    pub async fn next_code(&self, user_id: Uuid, secret: &Secret) -> String {
        let current = totp::step(Utc::now());
        let last = self.factor(user_id).await.unwrap().last_used_step;
        let step = last.map_or(current - 1, |last| (last + 1).max(current - 1));
        assert!(step <= current + 1, "no unused code within the clock skew");
        code_at(secret, step)
    }

    /// Moves when the sign-in `id` last authenticated back by `ago`.
    pub async fn authenticated_ago(&self, id: Uuid, ago: TimeDelta) {
        sign_in_session::Entity::update_many()
            .set(sign_in_session::ActiveModel {
                authenticated_at: Set(Utc::now() - ago),
                ..Default::default()
            })
            .filter(sign_in_session::Column::Id.eq(id))
            .exec(&self.db)
            .await
            .unwrap();
    }

    /// An account with `email` and [`PASSWORD`], unverified.
    pub async fn create_user(&self, email: &str) -> user::Model {
        create_user(&self.db, &self.passwords, email).await
    }

    /// The stored audit events of `action`, oldest first, as JSON objects.
    pub async fn events(&self, action: Action) -> Vec<Value> {
        let rows = self
            .db
            .query_all_raw(Statement::from_sql_and_values(
                DbBackend::Postgres,
                "SELECT to_jsonb(e) AS event FROM audit_event e WHERE action = $1 \
                 ORDER BY occurred_at",
                [action.to_value().into()],
            ))
            .await
            .unwrap();
        rows.iter()
            .map(|row| row.try_get("", "event").unwrap())
            .collect()
    }

    /// The emails sent to `address`, once `count` have arrived.
    pub async fn emails(&self, address: &str, count: usize) -> Vec<Value> {
        wait_until(&format!("{count} emails to {address}"), || async {
            self.mailpit.messages_to(address).await.len() >= count
        })
        .await;
        self.mailpit.messages_to(address).await
    }

    /// Every sign-in, oldest first.
    pub async fn sessions(&self) -> Vec<sign_in_session::Model> {
        let mut sessions = sign_in_session::Entity::find().all(&self.db).await.unwrap();
        sessions.sort_by_key(|session| session.created_at);
        sessions
    }

    pub async fn session(&self, id: Uuid) -> sign_in_session::Model {
        sign_in_session::Entity::find_by_id(id)
            .one(&self.db)
            .await
            .unwrap()
            .unwrap()
    }

    pub fn failed_metric(&self) -> u64 {
        self.metrics.sign_ins_failed.get()
    }
}

pub async fn create_user(
    db: &DatabaseConnection,
    passwords: &Passwords,
    email: &str,
) -> user::Model {
    let now = Utc::now();
    user::ActiveModel {
        email: Set(EmailAddress::new(email)),
        password_hash: Set(passwords.hash_new(&Password::new(PASSWORD)).await.unwrap()),
        display_name: Set("Ann Lee".to_owned()),
        accepted_terms_version: Set(legal::TERMS_VERSION.to_owned()),
        accepted_privacy_version: Set(legal::PRIVACY_VERSION.to_owned()),
        terms_accepted_at: Set(now),
        created_at: Set(now),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap()
}

/// The code of the app with `secret` for `step`, as typed.
pub fn code_at(secret: &Secret, step: i64) -> String {
    format!("{:06}", secret.code_at(step))
}

/// The secret of the app in the answer to `POST /api/mfa/setup`.
pub fn secret_of(setup: &Answer) -> Secret {
    let base32 = setup.json()["secret"].as_str().unwrap().to_owned();
    Secret::from_bytes(
        &data_encoding::BASE32_NOPAD
            .decode(base32.as_bytes())
            .unwrap(),
    )
    .unwrap()
}

/// A code that the app with `secret` doesn't make around now, so it is wrong.
pub fn wrong_code(secret: &Secret) -> String {
    let current = totp::step(Utc::now());
    let around: Vec<String> = (current - 3..=current + 3)
        .map(|step| code_at(secret, step))
        .collect();
    (0..)
        .map(|n| format!("{n:06}"))
        .find(|code| !around.contains(code))
        .unwrap()
}
