//! Sign-in against a real PostgreSQL and Mailpit (AUTH-05, AUTH-08, SEC-08, SEC-14): the cookie
//! it sets, the answers to wrong passwords and unknown addresses, and the throttling of failures.
//! The account endpoints run in-process, with each request's client address chosen by the test.
//! Needs Docker.

mod common;

use std::{
    net::{IpAddr, SocketAddr},
    sync::Arc,
    time::Duration,
};

use axum::{
    Router,
    body::Body,
    extract::ConnectInfo,
    http::{HeaderMap, Request, StatusCode, header},
};
use chrono::Utc;
use sea_orm::{
    ActiveEnum, ActiveModelTrait, ColumnTrait, ConnectionTrait, DatabaseConnection, DbBackend,
    EntityTrait, IntoActiveModel, QueryFilter, Set, Statement,
};
use serde_json::{Value, json};
use testcontainers_modules::{postgres::Postgres, testcontainers::ContainerAsync};
use tower::ServiceExt;
use twoviewer_server::{
    accounts::{
        Accounts, legal,
        password::{Password, Passwords},
        sign_in_session, token, user,
    },
    audit::Action,
    config::{self, PublicUrl},
    db::{self, EmailAddress, migrate},
    email::Mailer,
    http,
    metrics::Metrics,
    throttle,
};

use common::{
    LOCK_TIMEOUT,
    mailpit::{Mailpit, mailpit, wait_until},
    postgres,
};

const PASSWORD: &str = "violet otter harbor 92";
const WRONG: &str = "violet otter harbor 93";
const FIREFOX: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:133.0) Gecko/20100101 Firefox/133.0";

fn client() -> IpAddr {
    "203.0.113.7".parse().unwrap()
}

fn elsewhere() -> IpAddr {
    "198.51.100.20".parse().unwrap()
}

/// Settings that hash in well under a millisecond: these tests aren't about the cost.
fn cheap_hashes(config: &mut config::Config) {
    config.passwords.hash_memory_kib = 64;
    config.passwords.hash_iterations = 1;
}

/// The account endpoints, with the database and Mailpit they use.
struct App {
    router: Router,
    db: DatabaseConnection,
    mailpit: Mailpit,
    metrics: Arc<Metrics>,
    /// Hashes passwords with the same settings as the server.
    passwords: Passwords,
    _postgres: ContainerAsync<Postgres>,
}

/// The answer to a request: its status, headers and body.
#[derive(Debug)]
struct Answer {
    status: StatusCode,
    headers: HeaderMap,
    body: String,
}

impl Answer {
    fn json(&self) -> Value {
        serde_json::from_str(&self.body).unwrap()
    }

    /// The `name=value` of the cookie the answer sets.
    fn cookie(&self) -> String {
        let set_cookie = self.headers[header::SET_COOKIE].to_str().unwrap();
        set_cookie.split(';').next().unwrap().to_owned()
    }

    fn retry_after(&self) -> u64 {
        self.headers[header::RETRY_AFTER]
            .to_str()
            .unwrap()
            .parse()
            .unwrap()
    }
}

async fn app(configure: impl FnOnce(&mut config::Config)) -> App {
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
    async fn send(&self, mut request: Request<Body>, from: IpAddr) -> Answer {
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

    async fn sign_in_from(&self, email: &str, password: &str, from: IpAddr) -> Answer {
        let body = json!({ "email": email, "password": password });
        let request = Request::post("/api/sign-in")
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::USER_AGENT, FIREFOX)
            .body(Body::from(body.to_string()))
            .unwrap();
        self.send(request, from).await
    }

    async fn sign_in(&self, email: &str, password: &str) -> Answer {
        self.sign_in_from(email, password, client()).await
    }

    /// `GET /api/account`, with `cookie` if there is one.
    async fn account(&self, cookie: Option<&str>) -> Answer {
        let mut request = Request::get("/api/account").header(header::USER_AGENT, FIREFOX);
        if let Some(cookie) = cookie {
            request = request.header(header::COOKIE, cookie);
        }
        self.send(request.body(Body::empty()).unwrap(), client())
            .await
    }

    /// An account with `email` and [`PASSWORD`], unverified.
    async fn create_user(&self, email: &str) -> user::Model {
        create_user(&self.db, &self.passwords, email).await
    }

    /// The stored audit events of `action`, oldest first, as JSON objects.
    async fn events(&self, action: Action) -> Vec<Value> {
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
    async fn emails(&self, address: &str, count: usize) -> Vec<Value> {
        wait_until(&format!("{count} emails to {address}"), || async {
            self.mailpit.messages_to(address).await.len() >= count
        })
        .await;
        self.mailpit.messages_to(address).await
    }

    async fn sessions(&self) -> Vec<sign_in_session::Model> {
        sign_in_session::Entity::find().all(&self.db).await.unwrap()
    }

    fn failed_metric(&self) -> u64 {
        self.metrics.sign_ins_failed.get()
    }
}

async fn create_user(db: &DatabaseConnection, passwords: &Passwords, email: &str) -> user::Model {
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

#[tokio::test]
async fn sign_in_sets_a_cookie_that_identifies_the_user() {
    let app = app(|_| {}).await;
    let ann = app.create_user("ann@example.com").await;

    // Unverified accounts can sign in (AUTH-02), with the address as typed.
    let answer = app.sign_in(" Ann@Example.COM ", PASSWORD).await;

    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    assert_eq!(answer.body, "");
    // SEC-09: hidden from page scripts, for HTTPS only, and not sent by other sites.
    let set_cookie = answer.headers[header::SET_COOKIE].to_str().unwrap();
    let (name_value, attributes) = set_cookie.split_once("; ").unwrap();
    assert_eq!(
        attributes,
        "Path=/; Max-Age=2592000; Secure; HttpOnly; SameSite=Strict"
    );
    let token = name_value
        .strip_prefix("__Host-twoviewer-session=")
        .unwrap();
    assert_eq!(token.len(), 64);

    // The database has only the token's hash (SEC-07).
    let sessions = app.sessions().await;
    assert_eq!(sessions.len(), 1);
    let session = &sessions[0];
    assert_eq!(session.user_id, ann.id);
    assert_eq!(session.token_hash, token::hash(token));
    assert_eq!(session.ip_address.ip().to_string(), "203.0.113.7");
    assert_eq!(session.client_description, "Firefox on Linux");
    assert_eq!(session.revoked_at, None);
    assert_eq!(session.last_active_at, session.created_at);
    assert_eq!(
        session.expires_at - session.created_at,
        chrono::Duration::days(30)
    );

    // AUD-03.
    let events = app.events(Action::SignInSucceeded).await;
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event["actor_type"], "user");
    assert_eq!(event["actor_id"], json!(ann.id));
    assert_eq!(event["target_type"], "user");
    assert_eq!(event["target_id"], json!(ann.id));
    assert_eq!(event["outcome"], "success");
    assert_eq!(event["ip_address"], "203.0.113.7");
    assert_eq!(event["client_description"], "Firefox on Linux");
    assert_eq!(
        event["details"],
        json!({ "sign_in_session_id": session.id })
    );
    assert_eq!(app.failed_metric(), 0);

    // The cookie identifies the user.
    let account = app.account(Some(&answer.cookie())).await;
    assert_eq!(account.status, StatusCode::OK);
    assert_eq!(
        account.json(),
        json!({
            "id": ann.id,
            "email": "ann@example.com",
            "displayName": "Ann Lee",
            "emailVerified": false,
        })
    );
}

#[tokio::test]
async fn requests_without_an_active_sign_in_are_not_signed_in() {
    let app = app(|_| {}).await;
    app.create_user("ann@example.com").await;
    let cookie = app.sign_in("ann@example.com", PASSWORD).await.cookie();
    let not_signed_in = json!({ "error": "not_signed_in" });

    for cookie in [
        None,
        Some("theme=dark"),
        Some("__Host-twoviewer-session="),
        Some(&*format!("__Host-twoviewer-session={}", "a".repeat(64))),
        Some(&*format!("__Host-twoviewer-session={}", "a".repeat(10_000))),
    ] {
        let answer = app.account(cookie).await;
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED, "{cookie:?}");
        assert_eq!(answer.json(), not_signed_in);
    }

    // Expired, then revoked.
    let session = app.sessions().await.remove(0);
    let mut expired = session.clone().into_active_model();
    expired.expires_at = Set(Utc::now() - chrono::Duration::seconds(1));
    expired.update(&app.db).await.unwrap();
    assert_eq!(
        app.account(Some(&cookie)).await.status,
        StatusCode::UNAUTHORIZED
    );
    let mut revoked = session.into_active_model();
    revoked.expires_at = Set(Utc::now() + chrono::Duration::days(1));
    revoked.revoked_at = Set(Some(Utc::now()));
    revoked.update(&app.db).await.unwrap();
    assert_eq!(
        app.account(Some(&cookie)).await.status,
        StatusCode::UNAUTHORIZED
    );
}

#[tokio::test]
async fn a_wrong_password_and_an_unknown_address_get_the_same_answer() {
    let app = app(|_| {}).await;
    let ann = app.create_user("ann@example.com").await;

    let wrong = app.sign_in("ann@example.com", WRONG).await;
    let unknown = app.sign_in("nobody@example.com", PASSWORD).await;

    // SEC-14.
    for answer in [&wrong, &unknown] {
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
        assert_eq!(answer.json(), json!({ "error": "sign_in_failed" }));
        assert!(!answer.headers.contains_key(header::SET_COOKIE));
    }
    assert_eq!(wrong.body, unknown.body);
    let names = |answer: &Answer| {
        let mut names: Vec<_> = answer.headers.keys().map(|name| name.as_str()).collect();
        names.sort_unstable();
        names.join(",")
    };
    assert_eq!(names(&wrong), names(&unknown));
    assert!(app.sessions().await.is_empty());

    // Audited before the answer (AUD-03), by a visitor: whoever tried isn't known to be the
    // owner. The target says whether the address has an account.
    let events = app.events(Action::SignInFailed).await;
    assert_eq!(events.len(), 2);
    for (event, target) in events.iter().zip([json!(ann.id), Value::Null]) {
        assert_eq!(event["actor_type"], "user");
        assert_eq!(event["actor_id"], Value::Null);
        assert_eq!(event["ip_address"], "203.0.113.7");
        assert_eq!(event["client_description"], "Firefox on Linux");
        assert_eq!(event["outcome"], "failure");
        assert_eq!(event["reason_code"], "invalid_credentials");
        assert_eq!(event["target_id"], target);
        assert_eq!(event["details"], json!({}));
    }
    assert_eq!(app.failed_metric(), 2);
}

#[tokio::test]
async fn invalid_requests_are_rejected_without_counting() {
    let app = app(|_| {}).await;

    let answer = app.sign_in("  ", "").await;
    assert_eq!(answer.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        answer.json(),
        json!({
            "error": "invalid",
            "fields": { "email": { "code": "required" }, "password": { "code": "required" } },
        })
    );
    let answer = app.sign_in("not an address", PASSWORD).await;
    assert_eq!(
        answer.json(),
        json!({ "error": "invalid", "fields": { "email": { "code": "invalid" } } })
    );

    assert!(app.events(Action::SignInFailed).await.is_empty());
    assert!(
        throttle::Entity::find()
            .all(&app.db)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(app.failed_metric(), 0);
}

#[tokio::test]
async fn ten_failures_block_an_address_whether_or_not_it_has_an_account() {
    let app = app(|config| config.sign_in.block_duration = Duration::from_secs(3)).await;
    let ann = app.create_user("ann@example.com").await;

    for address in ["ann@example.com", "nobody@example.com"] {
        for attempt in 1..=10 {
            let answer = app.sign_in(address, WRONG).await;
            assert_eq!(
                answer.status,
                StatusCode::UNAUTHORIZED,
                "{address}, {attempt}"
            );
        }
    }

    // AUTH-08: blocked, even with the right password. An address without an account gets the
    // same answer (SEC-14).
    let blocked = app.sign_in("ann@example.com", PASSWORD).await;
    let unknown = app.sign_in("nobody@example.com", PASSWORD).await;
    for answer in [&blocked, &unknown] {
        assert_eq!(answer.status, StatusCode::TOO_MANY_REQUESTS);
        assert_eq!(answer.json(), json!({ "error": "too_many_requests" }));
        assert!((1..=3).contains(&answer.retry_after()), "{answer:?}");
    }
    // The address is blocked, not the network or other addresses.
    app.create_user("bob@example.com").await;
    assert_eq!(
        app.sign_in("bob@example.com", PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );

    // One block per address, audited when it starts.
    let blocks = app.events(Action::SignInBlocked).await;
    assert_eq!(blocks.len(), 2);
    for (event, target) in blocks.iter().zip([json!(ann.id), Value::Null]) {
        assert_eq!(event["actor_id"], Value::Null);
        assert_eq!(event["target_id"], target);
        assert_eq!(event["reason_code"], "too_many_failures");
        assert_eq!(event["details"], json!({ "blocked_seconds": 3 }));
    }
    let refused: Vec<_> = app
        .events(Action::SignInFailed)
        .await
        .into_iter()
        .filter(|event| event["reason_code"] == "blocked")
        .collect();
    assert_eq!(refused.len(), 2);
    assert_eq!(app.failed_metric(), 22);

    // The owner is emailed, and only the owner: no email goes to an address without an account.
    let emails = app.emails("ann@example.com", 1).await;
    assert_eq!(
        emails[0]["Subject"],
        "Sign-in to your TwoViewer account is paused"
    );
    let text = emails[0]["Text"].as_str().unwrap();
    assert!(text.contains("the wrong password 10 times"), "{text}");
    assert!(
        text.contains("https://twoviewer.test/reset-password"),
        "{text}"
    );
    assert!(!text.contains("203.0.113.7"), "{text}");
    assert_eq!(app.mailpit.messages().await.len(), 1);

    // The block ends.
    tokio::time::sleep(Duration::from_millis(3100)).await;
    assert_eq!(
        app.sign_in("ann@example.com", PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn failures_count_within_a_sliding_window_until_a_success() {
    let app = app(|config| config.sign_in.failure_window = Duration::from_secs(3)).await;
    app.create_user("ann@example.com").await;

    for _ in 0..9 {
        app.sign_in("ann@example.com", WRONG).await;
    }
    // A success forgets the failures.
    assert_eq!(
        app.sign_in("ann@example.com", PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );
    for _ in 0..9 {
        app.sign_in("ann@example.com", WRONG).await;
    }
    // Failures that have left the window don't count either.
    tokio::time::sleep(Duration::from_millis(3100)).await;
    for _ in 0..9 {
        assert_eq!(
            app.sign_in("ann@example.com", WRONG).await.status,
            StatusCode::UNAUTHORIZED
        );
    }
    assert_eq!(
        app.sign_in("ann@example.com", PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );
    assert!(app.events(Action::SignInBlocked).await.is_empty());
}

#[tokio::test]
async fn concurrent_failures_start_one_block() {
    let app = app(|_| {}).await;
    app.create_user("ann@example.com").await;

    let answers =
        futures_util::future::join_all((0..25).map(|_| app.sign_in("ann@example.com", WRONG)))
            .await;

    assert!(
        answers.iter().all(|answer| matches!(
            answer.status,
            StatusCode::UNAUTHORIZED | StatusCode::TOO_MANY_REQUESTS
        )),
        "{answers:?}"
    );
    assert_eq!(app.events(Action::SignInBlocked).await.len(), 1);
    assert_eq!(
        app.sign_in("ann@example.com", PASSWORD).await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
    app.emails("ann@example.com", 1).await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(app.mailpit.messages().await.len(), 1);
}

#[tokio::test]
async fn a_network_that_fails_too_often_is_refused() {
    let app = app(|config| config.sign_in.max_failures_per_network = 3).await;
    app.create_user("ann@example.com").await;

    // Guessing across addresses, which the limit per address doesn't catch (SEC-08).
    for address in ["a@example.com", "b@example.com", "c@example.com"] {
        assert_eq!(
            app.sign_in(address, WRONG).await.status,
            StatusCode::UNAUTHORIZED
        );
    }

    let refused = app.sign_in("ann@example.com", PASSWORD).await;
    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(refused.retry_after() >= 1);
    let event = app.events(Action::SignInFailed).await.pop().unwrap();
    assert_eq!(event["reason_code"], "network_blocked");
    // Other networks are not refused.
    assert_eq!(
        app.sign_in_from("ann@example.com", PASSWORD, elsewhere())
            .await
            .status,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn old_hashes_are_renewed_at_sign_in() {
    let app = app(|config| config.passwords.hash_iterations = 2).await;
    let mut older = config::Config::default();
    cheap_hashes(&mut older);
    let ann = create_user(
        &app.db,
        &Passwords::new(&older.passwords).unwrap(),
        "ann@example.com",
    )
    .await;

    assert_eq!(
        app.sign_in("ann@example.com", PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );

    let stored: String = app
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT password_hash FROM user_account WHERE id = $1",
            [ann.id.into()],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "password_hash")
        .unwrap();
    assert!(
        stored.starts_with("$argon2id$v=19$m=64,t=2,p=1$"),
        "{stored}"
    );
    // And the renewed hash works.
    assert_eq!(
        app.sign_in("ann@example.com", PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );
    let user = user::Entity::find()
        .filter(user::Column::Id.eq(ann.id))
        .one(&app.db)
        .await
        .unwrap()
        .unwrap();
    assert_ne!(user.password_hash, ann.password_hash);
}
