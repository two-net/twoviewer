//! Re-authentication before sensitive changes, against a real PostgreSQL and Mailpit (AUTH-10):
//! when it is needed, what it asks for, and how wrong attempts count. The account endpoints run
//! in-process. Needs Docker.

mod accounts;
mod common;

use axum::http::StatusCode;
use chrono::{TimeDelta, Utc};
use sea_orm::prelude::Uuid;
use serde_json::{Value, json};
use twoviewer_server::audit::Action;

use accounts::{App, PASSWORD, app, wrong_code};

const WRONG: &str = "violet otter harbor 93";

/// Signs `email` in, and returns the cookie and the ID of the new sign-in.
async fn signed_in(app: &App, email: &str) -> (String, Uuid) {
    let cookie = app.signed_in(email).await;
    let id = app.sessions().await.last().unwrap().id;
    (cookie, id)
}

/// Whether the sign-in of `cookie` may change MFA settings now, without re-authenticating.
async fn recently_authenticated(app: &App, cookie: &str) -> bool {
    let answer = app.mfa_setup(cookie).await;
    match answer.status {
        // With MFA on, setting up an app is refused, but only after the check.
        StatusCode::OK | StatusCode::CONFLICT => true,
        StatusCode::PRECONDITION_REQUIRED => {
            assert_eq!(answer.json()["error"], "reauthentication_required");
            false
        }
        status => panic!("unexpected {status}: {}", answer.body),
    }
}

#[tokio::test]
async fn sensitive_changes_need_the_password_within_15_minutes() {
    let app = app(|_| {}).await;
    let ann = app.create_user("ann@example.com").await;
    let (cookie, session_id) = signed_in(&app, "ann@example.com").await;
    let (other_cookie, other_id) = signed_in(&app, "ann@example.com").await;

    // Signing in counts.
    assert!(recently_authenticated(&app, &cookie).await);
    app.authenticated_ago(session_id, TimeDelta::minutes(14))
        .await;
    assert!(recently_authenticated(&app, &cookie).await);

    app.authenticated_ago(session_id, TimeDelta::minutes(15) + TimeDelta::seconds(5))
        .await;
    let answer = app.mfa_setup(&cookie).await;
    assert_eq!(answer.status, StatusCode::PRECONDITION_REQUIRED);
    assert_eq!(
        answer.json(),
        json!({ "error": "reauthentication_required", "secondFactor": false })
    );

    // A wrong password doesn't count.
    let answer = app.reauthenticate(&cookie, WRONG, None).await;
    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    assert_eq!(answer.json(), json!({ "error": "reauthentication_failed" }));
    assert!(!recently_authenticated(&app, &cookie).await);
    let events = app.events(Action::Reauthenticated).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["actor_id"], json!(ann.id));
    assert_eq!(events[0]["outcome"], "failure");
    assert_eq!(events[0]["reason_code"], "invalid_credentials");
    assert_eq!(
        events[0]["details"],
        json!({ "sign_in_session_id": session_id })
    );
    // Re-authentication isn't sign-in.
    assert_eq!(app.failed_metric(), 0);
    assert!(app.events(Action::SignInFailed).await.is_empty());

    // The right one does, for this sign-in only.
    app.authenticated_ago(other_id, TimeDelta::hours(1)).await;
    let answer = app.reauthenticate(&cookie, PASSWORD, None).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.body);
    assert!(Utc::now() - app.session(session_id).await.authenticated_at < TimeDelta::seconds(10));
    assert!(recently_authenticated(&app, &cookie).await);
    assert!(!recently_authenticated(&app, &other_cookie).await);

    let events = app.events(Action::Reauthenticated).await;
    assert_eq!(events.len(), 2);
    assert_eq!(events[1]["outcome"], "success");
    assert_eq!(events[1]["reason_code"], Value::Null);
    assert_eq!(
        events[1]["details"],
        json!({ "sign_in_session_id": session_id })
    );
}

#[tokio::test]
async fn with_mfa_on_reauthentication_needs_a_code_too() {
    let app = app(|_| {}).await;
    let ann = app.create_user("ann@example.com").await;
    let (cookie, session_id) = signed_in(&app, "ann@example.com").await;
    let (secret, codes) = app.enable_mfa(&cookie).await;
    app.authenticated_ago(session_id, TimeDelta::minutes(30))
        .await;

    let answer = app.mfa_disable(&cookie).await;
    assert_eq!(
        answer.json(),
        json!({ "error": "reauthentication_required", "secondFactor": true })
    );

    // The password alone isn't enough.
    let answer = app.reauthenticate(&cookie, PASSWORD, None).await;
    assert_eq!(answer.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        answer.json(),
        json!({ "error": "invalid", "fields": { "code": { "code": "required" } } })
    );
    let answer = app.reauthenticate(&cookie, "", Some("")).await;
    assert_eq!(
        answer.json(),
        json!({
            "error": "invalid",
            "fields": {
                "code": { "code": "required" },
                "password": { "code": "required" },
            },
        })
    );

    // A wrong code and a wrong password get the same answer, and the code of a wrong password
    // isn't used up.
    let code = app.next_code(ann.id, &secret).await;
    for (password, code) in [(PASSWORD, wrong_code(&secret)), (WRONG, code.clone())] {
        let answer = app.reauthenticate(&cookie, password, Some(&code)).await;
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
        assert_eq!(answer.json(), json!({ "error": "reauthentication_failed" }));
    }
    assert!(!recently_authenticated(&app, &cookie).await);

    let answer = app.reauthenticate(&cookie, PASSWORD, Some(&code)).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.body);
    let events = app.events(Action::Reauthenticated).await;
    assert_eq!(
        events.last().unwrap()["details"],
        json!({ "sign_in_session_id": session_id, "second_factor": "authenticator" })
    );

    // Recovery codes work here too.
    app.authenticated_ago(session_id, TimeDelta::minutes(30))
        .await;
    let answer = app.reauthenticate(&cookie, PASSWORD, Some(&codes[0])).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    let used = app.events(Action::RecoveryCodeUsed).await;
    assert_eq!(used.len(), 1);
    assert_eq!(used[0]["details"], json!({ "remaining": 9 }));
    assert!(recently_authenticated(&app, &cookie).await);
}

#[tokio::test]
async fn wrong_passwords_count_toward_the_sign_in_block() {
    let app = app(|config| config.sign_in.max_failures = 3).await;
    let ann = app.create_user("ann@example.com").await;
    let (cookie, _) = signed_in(&app, "ann@example.com").await;

    for _ in 0..3 {
        let answer = app.reauthenticate(&cookie, WRONG, None).await;
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    }

    // AUTH-08: the third starts the block, which is audited and emailed as at sign-in.
    let blocked = app.events(Action::SignInBlocked).await;
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0]["actor_id"], json!(ann.id));
    assert_eq!(blocked[0]["reason_code"], "too_many_failures");
    let emails = app.emails("ann@example.com", 1).await;
    assert_eq!(
        emails[0]["Subject"],
        "Sign-in to your TwoViewer account is paused"
    );

    // Both sign-in and re-authentication are refused, even with the right password.
    let answer = app.sign_in("ann@example.com", PASSWORD).await;
    assert_eq!(answer.status, StatusCode::TOO_MANY_REQUESTS);
    let answer = app.reauthenticate(&cookie, PASSWORD, None).await;
    assert_eq!(answer.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(answer.retry_after() > 800);
    let events = app.events(Action::Reauthenticated).await;
    assert_eq!(events.last().unwrap()["reason_code"], "blocked");
}

#[tokio::test]
async fn wrong_codes_count_toward_the_second_step_block() {
    let app = app(|config| config.mfa.max_failures = 2).await;
    let ann = app.create_user("ann@example.com").await;
    let (cookie, _) = signed_in(&app, "ann@example.com").await;
    let (secret, _) = app.enable_mfa(&cookie).await;

    for _ in 0..2 {
        let answer = app
            .reauthenticate(&cookie, PASSWORD, Some(&wrong_code(&secret)))
            .await;
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    }
    let blocked = app.events(Action::SignInBlocked).await;
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0]["reason_code"], "too_many_second_factor_failures");

    let code = app.next_code(ann.id, &secret).await;
    let answer = app.reauthenticate(&cookie, PASSWORD, Some(&code)).await;
    assert_eq!(answer.status, StatusCode::TOO_MANY_REQUESTS);
    // The second step of sign-in shares the block.
    let pending = app.sign_in("ann@example.com", PASSWORD).await.cookie();
    let answer = app.second_factor(Some(&pending), &code).await;
    assert_eq!(answer.status, StatusCode::TOO_MANY_REQUESTS);
}

#[tokio::test]
async fn reauthentication_needs_a_sign_in() {
    let app = app(|_| {}).await;
    app.create_user("ann@example.com").await;
    let (cookie, session_id) = signed_in(&app, "ann@example.com").await;

    let answer = app
        .send_json(
            axum::http::Method::POST,
            "/api/reauthenticate",
            None,
            json!({ "password": PASSWORD }),
        )
        .await;
    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    assert_eq!(answer.json(), json!({ "error": "not_signed_in" }));

    // Nor one that was revoked.
    app.sign_out(Some(&cookie)).await;
    let answer = app.reauthenticate(&cookie, PASSWORD, None).await;
    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    assert!(app.session(session_id).await.revoked_at.is_some());
}
