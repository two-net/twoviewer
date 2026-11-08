//! Password reset by email against a real PostgreSQL and Mailpit (AUTH-07, SEC-08, SEC-14): the
//! emailed link, the answers that don't reveal registered addresses, the limits on requests, and
//! what completing a reset does to the account's password, sign-ins and blocks. The account
//! endpoints run in-process, with each request's client address chosen by the test. Needs Docker.

mod accounts;
mod common;

use std::{net::IpAddr, time::Duration};

use axum::{
    body::Body,
    http::{Method, Request, StatusCode, header},
};
use chrono::Utc;
use sea_orm::{ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, Set};
use serde_json::{Value, json};
use twoviewer_server::{
    accounts::{
        email_link::{self, Purpose},
        pending_sign_in, user,
    },
    audit::Action,
    db::EmailAddress,
    http, throttle,
};

use accounts::{Answer, App, FIREFOX, PASSWORD, SAFARI, app, client, elsewhere};
use common::mailpit::wait_until;

const NEW_PASSWORD: &str = "amber lantern meadow 41";
const WRONG: &str = "violet otter harbor 93";
const SUBJECT: &str = "Reset your TwoViewer password";

/// `POST /api/password-reset` with `email`, from `from`.
async fn request_from(app: &App, email: &str, from: IpAddr) -> Answer {
    let request = Request::post("/api/password-reset")
        .header(header::CONTENT_TYPE, "application/json")
        .header(header::USER_AGENT, FIREFOX)
        .body(Body::from(json!({ "email": email }).to_string()))
        .unwrap();
    app.send(request, from).await
}

async fn request(app: &App, email: &str) -> Answer {
    request_from(app, email, client()).await
}

/// `POST /api/password-reset/check` with `token`.
async fn check(app: &App, token: &str) -> Answer {
    app.send_json(
        Method::POST,
        "/api/password-reset/check",
        None,
        json!({ "token": token }),
    )
    .await
}

/// `POST /api/password-reset/complete` with `token` and `password`.
async fn complete(app: &App, token: &str, password: &str) -> Answer {
    app.send_json(
        Method::POST,
        "/api/password-reset/complete",
        None,
        json!({ "token": token, "password": password }),
    )
    .await
}

/// The token of the reset link in `email`.
fn token(email: &Value) -> String {
    let text = email["Text"].as_str().unwrap();
    let link = text
        .lines()
        .find(|line| line.starts_with("https://twoviewer.test/reset-password#"))
        .unwrap_or_else(|| panic!("no link in {text}"));
    link.split_once('#').unwrap().1.to_owned()
}

/// Asks for a reset of `email`, and returns the token of the link in the `nth` email to it,
/// counting from 1.
async fn reset_link(app: &App, email: &str, nth: usize) -> String {
    let answer = request(app, email).await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.body);
    let emails = app.emails(email, nth).await;
    assert_eq!(emails[nth - 1]["Subject"], SUBJECT);
    token(&emails[nth - 1])
}

async fn user(app: &App, email: &str) -> user::Model {
    user::Entity::find()
        .filter(user::Column::Email.eq(EmailAddress::new(email)))
        .one(&app.db)
        .await
        .unwrap()
        .unwrap()
}

/// The reset links, oldest first.
async fn links(app: &App) -> Vec<email_link::Model> {
    let mut links = email_link::Entity::find()
        .filter(email_link::Column::Purpose.eq(Purpose::ResetPassword))
        .all(&app.db)
        .await
        .unwrap();
    links.sort_by_key(|link| link.created_at);
    links
}

/// Waits until `count` emails have been sent, and a little longer, so that none is on its way.
async fn sent_exactly(app: &App, count: u64) {
    wait_until(&format!("{count} emails sent"), || async {
        app.metrics.emails_sent.get() >= count
    })
    .await;
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(app.metrics.emails_sent.get(), count);
}

#[tokio::test]
async fn a_registered_address_gets_a_link_that_works_for_an_hour() {
    let app = app(|_| {}).await;
    let ann = app.create_user("ann@example.com").await;

    let answer = request(&app, "Ann@Example.com ").await;

    assert_eq!(answer.status, StatusCode::ACCEPTED);
    assert_eq!(answer.body, "");
    let emails = app.emails("ann@example.com", 1).await;
    assert_eq!(emails[0]["Subject"], SUBJECT);
    let text = emails[0]["Text"].as_str().unwrap();
    assert!(text.contains("The link works once, for 1 hour."), "{text}");
    // Never the display name, which whoever signed up chose.
    for body in ["Text", "HTML"] {
        assert!(!emails[0][body].as_str().unwrap().contains("Ann Lee"));
    }
    // The database has only the token's hash (SEC-07).
    let token = token(&emails[0]);
    let links = links(&app).await;
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].user_id, ann.id);
    assert_eq!(links[0].token_hash, email_link::hash(&token));
    assert_eq!(links[0].used_at, None);
    let lifetime = links[0].expires_at - Utc::now();
    assert!(
        (59 * 60..=60 * 60).contains(&lifetime.num_seconds()),
        "{lifetime}"
    );

    let events = app.events(Action::PasswordResetRequested).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["actor_type"], "user");
    assert_eq!(events[0]["actor_id"], Value::Null);
    assert_eq!(events[0]["ip_address"], "203.0.113.7");
    assert_eq!(events[0]["target_id"], json!(ann.id));
    assert_eq!(events[0]["outcome"], "success");
    assert_eq!(events[0]["details"], json!({ "emailed": true }));
}

/// SEC-14: an address without an account gets the same answer, and no email.
#[tokio::test]
async fn the_answer_does_not_reveal_registered_addresses() {
    let app = app(|_| {}).await;
    app.create_user("ann@example.com").await;

    let mut answers = Vec::new();
    for email in ["ann@example.com", "nobody@example.com"] {
        let mut answer = request(&app, email).await;
        answer.headers.remove(http::REQUEST_ID);
        answers.push((answer.status, answer.headers, answer.body));
    }

    assert_eq!(answers[0].0, StatusCode::ACCEPTED);
    assert_eq!(answers[0], answers[1]);
    sent_exactly(&app, 1).await;
    assert!(
        app.mailpit
            .messages_to("nobody@example.com")
            .await
            .is_empty()
    );
    assert_eq!(links(&app).await.len(), 1);
    let events = app.events(Action::PasswordResetRequested).await;
    assert_eq!(events.len(), 2);
    assert_eq!(events[1]["target_id"], Value::Null);
    assert_eq!(events[1]["outcome"], "success");
    assert_eq!(events[1]["details"], json!({ "emailed": false }));
}

#[tokio::test]
async fn an_address_gets_one_email_per_interval() {
    let app = app(|_| {}).await;
    app.create_user("ann@example.com").await;

    for _ in 0..3 {
        assert_eq!(
            request(&app, "ann@example.com").await.status,
            StatusCode::ACCEPTED
        );
    }

    sent_exactly(&app, 1).await;
    // No links were made for the emails that weren't sent.
    assert_eq!(links(&app).await.len(), 1);
    let emailed: Vec<_> = app
        .events(Action::PasswordResetRequested)
        .await
        .iter()
        .map(|event| event["details"]["emailed"].clone())
        .collect();
    assert_eq!(emailed, [json!(true), json!(false), json!(false)]);
}

/// SEC-08: a network that asks too often is refused for a while.
#[tokio::test]
async fn a_network_that_asks_too_often_is_refused() {
    let app = app(|config| config.password_reset.max_per_network = 2).await;
    let ann = app.create_user("ann@example.com").await;

    for email in ["a@example.com", "b@example.com"] {
        assert_eq!(request(&app, email).await.status, StatusCode::ACCEPTED);
    }
    let refused = request(&app, "ann@example.com").await;

    assert_eq!(refused.status, StatusCode::TOO_MANY_REQUESTS);
    assert_eq!(refused.json(), json!({ "error": "too_many_requests" }));
    assert!(
        (3590..=3600).contains(&refused.retry_after()),
        "{refused:?}"
    );
    assert!(links(&app).await.is_empty());
    let event = app
        .events(Action::PasswordResetRequested)
        .await
        .pop()
        .unwrap();
    assert_eq!(event["target_id"], json!(ann.id));
    assert_eq!(event["outcome"], "failure");
    assert_eq!(event["reason_code"], "network_blocked");
    // Other networks are not refused.
    assert_eq!(
        request_from(&app, "ann@example.com", elsewhere())
            .await
            .status,
        StatusCode::ACCEPTED
    );
    app.emails("ann@example.com", 1).await;
}

#[tokio::test]
async fn invalid_addresses_are_rejected() {
    let app = app(|_| {}).await;

    for (email, code) in [("not an address", "invalid"), (" ", "required")] {
        let answer = request(&app, email).await;

        assert_eq!(answer.status, StatusCode::UNPROCESSABLE_ENTITY, "{email}");
        assert_eq!(
            answer.json(),
            json!({ "error": "invalid", "fields": { "email": { "code": code } } })
        );
    }
    assert!(app.events(Action::PasswordResetRequested).await.is_empty());
}

#[tokio::test]
async fn checking_a_link_does_not_use_it() {
    let app = app(|_| {}).await;
    let ann = app.create_user("ann@example.com").await;
    let token = reset_link(&app, "ann@example.com", 1).await;

    for _ in 0..2 {
        let answer = check(&app, &token).await;
        assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
        assert_eq!(answer.json(), json!({ "email": "ann@example.com" }));
    }
    assert_eq!(links(&app).await[0].used_at, None);

    // Tokens that were never issued, and the token of a link for something else.
    let verification = email_link::create(
        &app.db,
        ann.id,
        Purpose::VerifyEmail,
        None,
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    for token in [
        "0123",
        &"a".repeat(64),
        &"a".repeat(10_000),
        "",
        verification.expose(),
    ] {
        for answer in [
            check(&app, token).await,
            complete(&app, token, NEW_PASSWORD).await,
        ] {
            assert_eq!(answer.status, StatusCode::GONE);
            assert_eq!(answer.json(), json!({ "error": "link_expired" }));
        }
    }
    assert_eq!(
        app.sign_in("ann@example.com", PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );
}

/// AUTH-07: the new password replaces the old one, and every sign-in of the account ends.
#[tokio::test]
async fn completing_a_reset_sets_the_password_and_signs_out_everywhere() {
    let app = app(|_| {}).await;
    let ann = app.create_user("ann@example.com").await;
    let bob = app.create_user("bob@example.com").await;
    let firefox = app.signed_in("ann@example.com").await;
    let safari = app
        .sign_in_with("ann@example.com", PASSWORD, elsewhere(), SAFARI)
        .await
        .cookie();
    let bobs = app.signed_in("bob@example.com").await;
    let token = reset_link(&app, "ann@example.com", 1).await;

    let answer = complete(&app, &token, NEW_PASSWORD).await;

    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.body);
    assert_eq!(answer.body, "");
    // It doesn't sign in.
    assert_eq!(answer.set_cookie(), None);
    assert!(!app.is_signed_in(&firefox).await);
    assert!(!app.is_signed_in(&safari).await);
    // Other accounts are untouched.
    assert!(app.is_signed_in(&bobs).await);
    assert_eq!(user(&app, "bob@example.com").await, bob);

    let revoked = app.events(Action::SignInSessionRevoked).await;
    assert_eq!(revoked.len(), 2);
    let sessions = app.sessions().await;
    for (event, session) in revoked.iter().zip(&sessions) {
        assert_eq!(event["actor_id"], json!(ann.id));
        assert_eq!(event["target_id"], json!(ann.id));
        assert!(session.revoked_at.is_some());
    }
    let completed = app.events(Action::PasswordResetCompleted).await;
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0]["actor_id"], json!(ann.id));
    assert_eq!(completed[0]["target_id"], json!(ann.id));
    assert_eq!(completed[0]["outcome"], "success");
    assert_eq!(completed[0]["correlation_id"], revoked[0]["correlation_id"]);

    assert_eq!(
        app.sign_in("ann@example.com", PASSWORD).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.sign_in("ann@example.com", NEW_PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );

    // The link works once.
    assert_eq!(check(&app, &token).await.status, StatusCode::GONE);
    let again = complete(&app, &token, "another new password 5").await;
    assert_eq!(again.status, StatusCode::GONE);
    assert_eq!(app.events(Action::PasswordResetCompleted).await.len(), 1);
    assert_eq!(
        app.sign_in("ann@example.com", NEW_PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );
}

/// The link proves that its owner received it, as the verification link would (AUTH-02).
#[tokio::test]
async fn a_reset_verifies_the_address_once() {
    let app = app(|config| config.password_reset.email_interval = Duration::ZERO).await;
    let ann = app.create_user("ann@example.com").await;
    assert!(!ann.is_verified());

    let token = reset_link(&app, "ann@example.com", 1).await;
    assert_eq!(
        complete(&app, &token, NEW_PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );

    let verified = user(&app, "ann@example.com").await;
    assert!(verified.is_verified());
    let events = app.events(Action::EmailVerified).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["actor_id"], json!(ann.id));

    // A second reset leaves it as it is.
    let token = reset_link(&app, "ann@example.com", 2).await;
    assert_eq!(
        complete(&app, &token, "another new password 5")
            .await
            .status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(app.events(Action::EmailVerified).await.len(), 1);
    assert_eq!(
        user(&app, "ann@example.com").await.email_verified_at,
        verified.email_verified_at
    );
}

#[tokio::test]
async fn a_reset_uses_up_every_link_of_the_account() {
    let app = app(|config| config.password_reset.email_interval = Duration::ZERO).await;
    app.create_user("ann@example.com").await;
    let first = reset_link(&app, "ann@example.com", 1).await;
    let second = reset_link(&app, "ann@example.com", 2).await;

    // Earlier links keep working until one of them is used.
    assert_eq!(check(&app, &first).await.status, StatusCode::OK);
    assert_eq!(
        complete(&app, &second, NEW_PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );

    assert_eq!(
        complete(&app, &first, "another new password 5")
            .await
            .status,
        StatusCode::GONE
    );
    assert!(links(&app).await.iter().all(|link| link.used_at.is_some()));
    assert_eq!(
        app.sign_in("ann@example.com", NEW_PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );
}

/// AUTH-03, AUTH-04: a password that breaks the rules is refused, and the link still works.
#[tokio::test]
async fn rejected_passwords_leave_the_link_working() {
    let app = app(|_| {}).await;
    app.create_user("ann@example.com").await;
    let token = reset_link(&app, "ann@example.com", 1).await;

    for (password, error) in [
        ("", json!({ "code": "required" })),
        ("short", json!({ "code": "too_short", "min": 12 })),
        ("password1234", json!({ "code": "compromised" })),
    ] {
        let answer = complete(&app, &token, password).await;

        assert_eq!(
            answer.status,
            StatusCode::UNPROCESSABLE_ENTITY,
            "{password}"
        );
        assert_eq!(
            answer.json(),
            json!({ "error": "invalid", "fields": { "password": error } })
        );
    }

    assert_eq!(check(&app, &token).await.status, StatusCode::OK);
    assert!(app.events(Action::PasswordResetCompleted).await.is_empty());
    assert_eq!(
        complete(&app, &token, NEW_PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );
}

#[tokio::test]
async fn expired_links_do_not_reset() {
    let app = app(|_| {}).await;
    app.create_user("ann@example.com").await;
    let token = reset_link(&app, "ann@example.com", 1).await;
    let mut link = links(&app).await.remove(0).into_active_model();
    link.expires_at = Set(Utc::now() - chrono::Duration::seconds(1));
    link.update(&app.db).await.unwrap();

    assert_eq!(check(&app, &token).await.status, StatusCode::GONE);
    // The link is checked first: a password that breaks the rules makes no difference.
    for password in [NEW_PASSWORD, "short"] {
        assert_eq!(
            complete(&app, &token, password).await.status,
            StatusCode::GONE
        );
    }
    assert!(app.events(Action::PasswordResetCompleted).await.is_empty());
    assert_eq!(
        app.sign_in("ann@example.com", PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );
}

/// The email about a block (AUTH-08) points to the reset, after which the owner can sign in.
#[tokio::test]
async fn a_reset_lifts_the_block_on_sign_in() {
    let app = app(|_| {}).await;
    app.create_user("ann@example.com").await;
    for _ in 0..10 {
        app.sign_in("ann@example.com", WRONG).await;
    }
    assert_eq!(
        app.sign_in("ann@example.com", PASSWORD).await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
    // The first email says that sign-in is paused.
    let token = reset_link(&app, "ann@example.com", 2).await;

    assert_eq!(
        complete(&app, &token, NEW_PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );

    assert!(
        throttle::Entity::find()
            .all(&app.db)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        app.sign_in("ann@example.com", NEW_PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );
}

/// A reset doesn't get past MFA: signing in still takes a code, and sign-ins waiting for one,
/// which were started with the old password, end.
#[tokio::test]
async fn with_mfa_signing_in_after_a_reset_still_takes_a_code() {
    let app = app(|_| {}).await;
    let ann = app.create_user("ann@example.com").await;
    let cookie = app.signed_in("ann@example.com").await;
    let (secret, _) = app.enable_mfa(&cookie).await;
    let pending = app.sign_in("ann@example.com", PASSWORD).await;
    assert_eq!(pending.status, StatusCode::PRECONDITION_REQUIRED);
    let token = reset_link(&app, "ann@example.com", 1).await;

    assert_eq!(
        complete(&app, &token, NEW_PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );

    assert!(
        pending_sign_in::Entity::find()
            .all(&app.db)
            .await
            .unwrap()
            .is_empty()
    );
    let code = app.next_code(ann.id, &secret).await;
    let answer = app.second_factor(Some(&pending.cookie()), &code).await;
    assert_eq!(answer.status, StatusCode::GONE);
    assert_eq!(answer.json(), json!({ "error": "sign_in_step_expired" }));

    let answer = app.sign_in("ann@example.com", NEW_PASSWORD).await;
    assert_eq!(answer.status, StatusCode::PRECONDITION_REQUIRED);
    assert_eq!(answer.json(), json!({ "error": "second_factor_required" }));
    // MFA is still on.
    assert!(app.factor(ann.id).await.unwrap().confirmed_at.is_some());
}
