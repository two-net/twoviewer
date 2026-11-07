//! Sign-up and email verification against a real PostgreSQL and Mailpit (AUTH-01, AUTH-02,
//! AUTH-15, SEC-14). The account endpoints run in-process, with each request's client address
//! chosen by the test. Needs Docker.

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
    accounts::{Accounts, challenge, email_link, legal, password::Passwords, user},
    audit::Action,
    config::{self, PublicUrl},
    db::{self, EmailAddress, migrate},
    email::Mailer,
    http,
    metrics::Metrics,
};

use common::{
    LOCK_TIMEOUT,
    mailpit::{Mailpit, mailpit, wait_until},
    postgres,
};

const PASSWORD: &str = "violet otter harbor 92";
const FIREFOX: &str = "Mozilla/5.0 (X11; Linux x86_64; rv:133.0) Gecko/20100101 Firefox/133.0";

fn client() -> IpAddr {
    "203.0.113.7".parse().unwrap()
}

/// The account endpoints, with the database and Mailpit they use.
struct App {
    router: Router,
    db: DatabaseConnection,
    mailpit: Mailpit,
    metrics: Arc<Metrics>,
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
}

async fn app(configure: impl FnOnce(&mut config::Config)) -> App {
    let mut config = config::Config::default();
    // Cheap hashes: these tests aren't about their cost.
    config.passwords.hash_memory_kib = 64;
    config.passwords.hash_iterations = 1;
    configure(&mut config);

    let ((postgres, url), mailpit) = tokio::join!(postgres(), mailpit());
    migrate::migrate(&url, LOCK_TIMEOUT).await.unwrap();
    let db = db::connect(&url).await.unwrap();
    let metrics = Arc::new(Metrics::new());
    let mailer = Mailer::new(&mailpit.config(), None, metrics.clone()).unwrap();
    let accounts = Accounts::new(
        db.clone(),
        mailer,
        Passwords::new(&config.passwords).unwrap(),
        PublicUrl::parse("https://twoviewer.test").unwrap(),
        &config,
        metrics.clone(),
    );
    App {
        router: http::router(accounts.routes(), metrics.clone(), Duration::ZERO),
        db,
        mailpit,
        metrics,
        _postgres: postgres,
    }
}

fn sign_up_body(email: &str, password: &str) -> Value {
    json!({
        "email": email,
        "displayName": "  Ann Lee ",
        "password": password,
        "termsVersion": legal::TERMS_VERSION,
        "privacyVersion": legal::PRIVACY_VERSION,
    })
}

impl App {
    async fn post(&self, path: &str, body: &Value, from: IpAddr) -> Answer {
        let mut request = Request::post(path)
            .header(header::CONTENT_TYPE, "application/json")
            .header(header::USER_AGENT, FIREFOX)
            .body(Body::from(body.to_string()))
            .unwrap();
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

    async fn sign_up(&self, email: &str) -> Answer {
        self.post("/api/sign-up", &sign_up_body(email, PASSWORD), client())
            .await
    }

    async fn verify(&self, token: &str) -> Answer {
        self.post("/api/verify-email", &json!({ "token": token }), client())
            .await
    }

    async fn user(&self, email: &str) -> Option<user::Model> {
        user::Entity::find()
            .filter(user::Column::Email.eq(EmailAddress::new(email)))
            .one(&self.db)
            .await
            .unwrap()
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

    /// The token of the verification link in `email`.
    fn token(email: &Value) -> String {
        let text = email["Text"].as_str().unwrap();
        let link = text
            .lines()
            .find(|line| line.starts_with("https://twoviewer.test/verify-email#"))
            .unwrap_or_else(|| panic!("no link in {text}"));
        link.split_once('#').unwrap().1.to_owned()
    }

    /// Signs `email` up and returns the token emailed to it.
    async fn signed_up(&self, email: &str) -> String {
        assert_eq!(self.sign_up(email).await.status, StatusCode::ACCEPTED);
        Self::token(&self.emails(email, 1).await[0])
    }
}

#[tokio::test]
async fn sign_up_creates_an_unverified_account_and_emails_a_link() {
    let app = app(|_| {}).await;

    let answer = app.sign_up("Ann.Lee@Example.com").await;

    assert_eq!(answer.status, StatusCode::ACCEPTED);
    assert_eq!(answer.body, "");
    let user = app.user("ann.lee@example.com").await.unwrap();
    assert_eq!(user.email.as_str(), "ann.lee@example.com");
    assert_eq!(user.display_name, "Ann Lee");
    assert!(!user.is_verified());
    assert_eq!(user.accepted_terms_version, legal::TERMS_VERSION);
    assert_eq!(user.accepted_privacy_version, legal::PRIVACY_VERSION);
    let stored: String = app
        .db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "SELECT password_hash FROM user_account",
            [],
        ))
        .await
        .unwrap()
        .unwrap()
        .try_get("", "password_hash")
        .unwrap();
    assert!(stored.starts_with("$argon2id$"), "{stored}");
    assert!(!stored.contains(PASSWORD));

    // AUTH-01, AUD-03: sign-up and the accepted versions are audited.
    for (action, details) in [
        (Action::SignUp, json!({})),
        (
            Action::TermsAccepted,
            json!({
                "terms_version": legal::TERMS_VERSION,
                "privacy_version": legal::PRIVACY_VERSION,
            }),
        ),
    ] {
        let events = app.events(action).await;
        assert_eq!(events.len(), 1, "{action:?}");
        let event = &events[0];
        assert_eq!(event["actor_type"], "user");
        assert_eq!(event["actor_id"], json!(user.id));
        assert_eq!(event["target_type"], "user");
        assert_eq!(event["target_id"], json!(user.id));
        assert_eq!(event["outcome"], "success");
        assert_eq!(event["ip_address"], "203.0.113.7");
        assert_eq!(event["client_description"], "Firefox on Linux");
        assert_eq!(event["details"], details);
    }

    // AUTH-02: the email has a link, and the database only the hash of its token.
    let emails = app.emails("ann.lee@example.com", 1).await;
    assert_eq!(
        emails[0]["Subject"],
        "Verify your email address for TwoViewer"
    );
    let token = App::token(&emails[0]);
    assert_eq!(token.len(), 64);
    let links = email_link::Entity::find().all(&app.db).await.unwrap();
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].user_id, user.id);
    assert_eq!(links[0].purpose, email_link::Purpose::VerifyEmail);
    assert_eq!(links[0].token_hash, email_link::hash(&token));
    assert_eq!(links[0].used_at, None);
    let lifetime = links[0].expires_at - Utc::now();
    assert!(
        lifetime > chrono::Duration::hours(23) && lifetime <= chrono::Duration::hours(24),
        "{lifetime}"
    );
    // The display name, which whoever signs up chooses, isn't in the email.
    assert!(!emails[0]["Text"].as_str().unwrap().contains("Ann Lee"));
    assert!(!emails[0]["HTML"].as_str().unwrap().contains("Ann Lee"));
}

#[tokio::test]
async fn the_link_verifies_the_address_once() {
    let app = app(|_| {}).await;
    let token = app.signed_up("ann@example.com").await;

    let answer = app.verify(&token).await;

    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    let user = app.user("ann@example.com").await.unwrap();
    assert!(user.is_verified());
    let link = email_link::Entity::find()
        .one(&app.db)
        .await
        .unwrap()
        .unwrap();
    assert!(link.used_at.is_some());
    let events = app.events(Action::EmailVerified).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["actor_id"], json!(user.id));
    assert_eq!(events[0]["target_id"], json!(user.id));
    assert_eq!(events[0]["details"], json!({}));

    // Opened again, say after a mail scanner, the link still reports the address as verified, but
    // verifies nothing.
    assert_eq!(app.verify(&token).await.status, StatusCode::NO_CONTENT);
    assert_eq!(app.events(Action::EmailVerified).await.len(), 1);
    assert_eq!(
        app.user("ann@example.com").await.unwrap().email_verified_at,
        user.email_verified_at
    );

    for token in ["0123", &"a".repeat(64), &"a".repeat(10_000), ""] {
        let answer = app.verify(token).await;
        assert_eq!(answer.status, StatusCode::GONE);
        assert_eq!(answer.json(), json!({ "error": "link_expired" }));
    }
}

#[tokio::test]
async fn expired_links_do_not_verify() {
    let app = app(|_| {}).await;
    let token = app.signed_up("ann@example.com").await;
    let mut link = email_link::Entity::find()
        .one(&app.db)
        .await
        .unwrap()
        .unwrap()
        .into_active_model();
    link.expires_at = Set(Utc::now() - chrono::Duration::seconds(1));
    link.update(&app.db).await.unwrap();

    let answer = app.verify(&token).await;

    assert_eq!(answer.status, StatusCode::GONE);
    assert!(!app.user("ann@example.com").await.unwrap().is_verified());
    assert!(app.events(Action::EmailVerified).await.is_empty());
}

/// SEC-14: the answer is the same whether the address is new, has an unverified account, or has
/// a verified one. Only the owner of the address learns which, from the email.
#[tokio::test]
async fn sign_up_does_not_reveal_registered_addresses() {
    let app = app(|config| {
        config.sign_up.notice_interval = Duration::ZERO;
        config.sign_up.challenge_after = 10;
    })
    .await;
    let token = app.signed_up("verified@example.com").await;
    assert_eq!(app.verify(&token).await.status, StatusCode::NO_CONTENT);
    app.signed_up("unverified@example.com").await;
    let before = (
        app.user("verified@example.com").await.unwrap(),
        app.user("unverified@example.com").await.unwrap(),
    );

    let mut answers = Vec::new();
    for email in [
        "new@example.com",
        "verified@example.com",
        "unverified@example.com",
    ] {
        let mut body = sign_up_body(email, "a different password 7");
        body["displayName"] = json!("Mallory");
        let mut answer = app.post("/api/sign-up", &body, client()).await;
        answer.headers.remove(http::REQUEST_ID);
        answers.push((answer.status, answer.headers, answer.body));
    }

    assert_eq!(answers[0].0, StatusCode::ACCEPTED);
    assert_eq!(answers[0], answers[1]);
    assert_eq!(answers[0], answers[2]);

    // The existing accounts are unchanged, and no new ones were made.
    assert_eq!(app.user("verified@example.com").await.unwrap(), before.0);
    assert_eq!(app.user("unverified@example.com").await.unwrap(), before.1);
    assert_eq!(app.events(Action::SignUp).await.len(), 3);

    let new = app.emails("new@example.com", 1).await;
    assert_eq!(new[0]["Subject"], "Verify your email address for TwoViewer");
    let verified = app.emails("verified@example.com", 2).await;
    assert_eq!(
        verified[1]["Subject"],
        "You already have a TwoViewer account"
    );
    let text = verified[1]["Text"].as_str().unwrap();
    for url in [
        "https://twoviewer.test/sign-in",
        "https://twoviewer.test/reset-password",
    ] {
        assert!(text.lines().any(|line| line == url), "{text}");
    }
    // A repeated sign-up verifies the existing account, with the password chosen first.
    let unverified = app.emails("unverified@example.com", 2).await;
    let text = unverified[1]["Text"].as_str().unwrap();
    assert!(text.contains("signed up for TwoViewer again"), "{text}");
    assert_eq!(
        app.verify(&App::token(&unverified[1])).await.status,
        StatusCode::NO_CONTENT
    );
    let user = app.user("unverified@example.com").await.unwrap();
    assert!(user.is_verified());
    assert_eq!(user.password_hash, before.1.password_hash);
}

#[tokio::test]
async fn sign_up_emails_an_address_at_most_once_per_interval() {
    let app = app(|_| {}).await;

    assert_eq!(
        app.sign_up("ann@example.com").await.status,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        app.sign_up("ann@example.com").await.status,
        StatusCode::ACCEPTED
    );
    assert_eq!(
        app.sign_up("ann@example.com").await.status,
        StatusCode::ACCEPTED
    );

    wait_until("an email sent", || async {
        app.metrics.emails_sent.get() >= 1
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(app.metrics.emails_sent.get(), 1);
    assert_eq!(app.mailpit.messages_to("ann@example.com").await.len(), 1);
    // No links were made for the emails that weren't sent.
    assert_eq!(
        email_link::Entity::find().all(&app.db).await.unwrap().len(),
        1
    );
}

/// AUTH-15: a network that signs up often must solve a proof-of-work challenge first.
#[tokio::test]
async fn busy_networks_must_solve_a_challenge() {
    let app = app(|config| {
        config.sign_up.challenge_after = 0;
        config.sign_up.challenge_difficulty = 8;
    })
    .await;

    let answer = app.sign_up("ann@example.com").await;

    assert_eq!(answer.status, StatusCode::PRECONDITION_REQUIRED);
    let body = answer.json();
    assert_eq!(body["error"], "challenge_required");
    assert_eq!(body["challenge"]["difficulty"], 8);
    assert!(app.user("ann@example.com").await.is_none());
    let id = body["challenge"]["id"].as_str().unwrap().parse().unwrap();
    let nonce = (0..)
        .find(|nonce| challenge::solves(id, *nonce, 8))
        .unwrap();
    let solved = |nonce: u64| {
        let mut body = sign_up_body("ann@example.com", PASSWORD);
        body["challenge"] = json!({ "id": id, "nonce": nonce });
        body
    };

    let answer = app.post("/api/sign-up", &solved(nonce), client()).await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.body);
    assert!(app.user("ann@example.com").await.is_some());

    // A challenge can be used only once.
    let answer = app.post("/api/sign-up", &solved(nonce), client()).await;
    assert_eq!(answer.status, StatusCode::PRECONDITION_REQUIRED);
    // A wrong solution gets a new challenge, and uses up the old one.
    let id = answer.json()["challenge"]["id"]
        .as_str()
        .unwrap()
        .to_owned();
    let mut wrong = sign_up_body("ann@example.com", PASSWORD);
    let nonce = (0..)
        .find(|nonce| !challenge::solves(id.parse().unwrap(), *nonce, 8))
        .unwrap();
    wrong["challenge"] = json!({ "id": id, "nonce": nonce });
    let answer = app.post("/api/sign-up", &wrong, client()).await;
    assert_eq!(answer.status, StatusCode::PRECONDITION_REQUIRED);
    assert_ne!(answer.json()["challenge"]["id"], json!(id));
    // Only the newest challenge is left: the others were used up.
    assert_eq!(
        challenge::Entity::find().all(&app.db).await.unwrap().len(),
        1
    );
}

#[tokio::test]
async fn networks_are_refused_past_the_maximum() {
    let app = app(|config| {
        config.sign_up.challenge_after = 1;
        config.sign_up.max_per_network = 1;
    })
    .await;

    assert_eq!(
        app.sign_up("ann@example.com").await.status,
        StatusCode::ACCEPTED
    );
    let answer = app.sign_up("bob@example.com").await;

    assert_eq!(answer.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(answer.json(), json!({ "error": "too_many_requests" }));
    let retry_after: u64 = answer.headers[header::RETRY_AFTER]
        .to_str()
        .unwrap()
        .parse()
        .unwrap();
    assert!((3590..=3600).contains(&retry_after), "{retry_after}");
    assert!(app.user("bob@example.com").await.is_none());
    // Other networks may still sign up.
    let other = "2001:db8::1".parse().unwrap();
    let answer = app
        .post(
            "/api/sign-up",
            &sign_up_body("bob@example.com", PASSWORD),
            other,
        )
        .await;
    assert_eq!(answer.status, StatusCode::ACCEPTED);
}

#[tokio::test]
async fn invalid_sign_ups_list_every_problem() {
    let app = app(|config| config.accounts.display_name_max_length = 10).await;

    for (changes, fields) in [
        (
            json!({ "email": "not an address", "displayName": "  ", "password": "short",
                    "termsVersion": "", "privacyVersion": "" }),
            json!({
                "email": { "code": "invalid" },
                "displayName": { "code": "required" },
                "password": { "code": "too_short", "min": 12 },
                "terms": { "code": "required" },
            }),
        ),
        (
            json!({ "email": " ", "displayName": "Ann Lee-Smith", "password": "password1234" }),
            json!({
                "email": { "code": "required" },
                "displayName": { "code": "too_long", "max": 10 },
                "password": { "code": "compromised" },
            }),
        ),
        (
            json!({ "displayName": "Ann\u{0007}" }),
            json!({ "displayName": { "code": "invalid" } }),
        ),
    ] {
        let mut body = sign_up_body("ann@example.com", PASSWORD);
        body.as_object_mut()
            .unwrap()
            .extend(changes.as_object().unwrap().clone());

        let answer = app.post("/api/sign-up", &body, client()).await;

        assert_eq!(answer.status, StatusCode::UNPROCESSABLE_ENTITY, "{changes}");
        assert_eq!(
            answer.json(),
            json!({ "error": "invalid", "fields": fields }),
            "{changes}"
        );
    }

    // Texts the user accepted, that have changed since.
    let mut outdated = sign_up_body("ann@example.com", PASSWORD);
    outdated["termsVersion"] = json!("2020-01-01");
    let answer = app.post("/api/sign-up", &outdated, client()).await;
    assert_eq!(answer.status, StatusCode::CONFLICT);
    assert_eq!(answer.json(), json!({ "error": "terms_changed" }));

    // Not the JSON the endpoint expects.
    let answer = app
        .post(
            "/api/sign-up",
            &json!({ "email": "ann@example.com" }),
            client(),
        )
        .await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert_eq!(answer.json(), json!({ "error": "malformed" }));

    assert!(app.user("ann@example.com").await.is_none());
    assert!(app.mailpit.messages().await.is_empty());
}
