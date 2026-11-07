//! MFA with an authenticator app against a real PostgreSQL and Mailpit (AUTH-05, AUTH-06, SEC-07,
//! SEC-08): turning it on and off, the second step of sign-in, recovery codes, and the throttling
//! of wrong codes. The account endpoints run in-process. Needs Docker.

mod accounts;
mod common;

use axum::http::{StatusCode, header};
use chrono::{TimeDelta, Utc};
use sea_orm::{ColumnTrait, EntityTrait, QueryFilter, Set, prelude::Uuid};
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use twoviewer_server::{
    accounts::{
        mfa::{cipher::Cipher, recovery_code},
        pending_sign_in, token,
    },
    audit::Action,
    throttle,
};

use accounts::{App, PASSWORD, app, code_at, secret_of, wrong_code};
use common::mfa_key;

/// Signs `email` in, and returns the cookie and the ID of the new sign-in.
async fn signed_in(app: &App, email: &str) -> (String, Uuid) {
    let cookie = app.signed_in(email).await;
    let id = app.sessions().await.last().unwrap().id;
    (cookie, id)
}

/// The cookie of the pending sign-in that the password step of `email` starts.
async fn password_step(app: &App, email: &str) -> String {
    let answer = app.sign_in(email, PASSWORD).await;
    assert_eq!(
        answer.status,
        StatusCode::PRECONDITION_REQUIRED,
        "{}",
        answer.body
    );
    assert_eq!(answer.json(), json!({ "error": "second_factor_required" }));
    answer.cookie()
}

/// The `Set-Cookie` headers of an answer.
fn set_cookies(answer: &accounts::Answer) -> Vec<String> {
    answer
        .headers
        .get_all(header::SET_COOKIE)
        .iter()
        .map(|value| value.to_str().unwrap().to_owned())
        .collect()
}

#[tokio::test]
async fn turning_mfa_on_needs_a_recent_sign_in_and_a_code_from_the_app() {
    let app = app(|_| {}).await;
    let ann = app.create_user("ann@example.com").await;
    let (cookie, session_id) = signed_in(&app, "ann@example.com").await;

    assert_eq!(
        app.mfa(&cookie).await.json(),
        json!({ "enabled": false, "recoveryCodesLeft": 0 })
    );

    // AUTH-10: changing MFA settings needs the password within 15 minutes.
    app.authenticated_ago(session_id, TimeDelta::minutes(16))
        .await;
    let answer = app.mfa_setup(&cookie).await;
    assert_eq!(answer.status, StatusCode::PRECONDITION_REQUIRED);
    assert_eq!(
        answer.json(),
        json!({ "error": "reauthentication_required", "secondFactor": false })
    );
    assert!(app.factor(ann.id).await.is_none());

    app.authenticated_ago(session_id, TimeDelta::minutes(14))
        .await;
    let setup = app.mfa_setup(&cookie).await;
    assert_eq!(setup.status, StatusCode::OK, "{}", setup.body);
    let body = setup.json();
    let base32 = body["secret"].as_str().unwrap();
    assert_eq!(base32.len(), 32);
    assert_eq!(
        body["uri"],
        format!(
            "otpauth://totp/TwoViewer:ann%40example.com?secret={base32}&issuer=TwoViewer\
             &algorithm=SHA1&digits=6&period=30"
        )
    );
    let size = body["qrCode"]["size"].as_u64().unwrap();
    assert!(size >= 21 + 8, "{size}");
    assert!(body["qrCode"]["path"].as_str().unwrap().starts_with('M'));
    let secret = secret_of(&setup);

    // SEC-07: the secret is stored encrypted for Ann, and MFA isn't on until confirmed.
    let factor = app.factor(ann.id).await.unwrap();
    assert_eq!(factor.confirmed_at, None);
    assert!(
        !factor
            .secret_encrypted
            .windows(secret.expose().len())
            .any(|window| window == secret.expose())
    );
    assert!(
        !factor
            .secret_encrypted
            .windows(base32.len())
            .any(|window| window == base32.as_bytes())
    );
    let decrypted = Cipher::new(&mfa_key())
        .decrypt(ann.id, &factor.secret_encrypted)
        .unwrap();
    assert_eq!(decrypted.expose(), secret.expose());
    assert_eq!(app.mfa(&cookie).await.json()["enabled"], false);

    // A wrong code doesn't turn it on.
    for (code, reason) in [
        (wrong_code(&secret), "invalid"),
        (" ".to_owned(), "required"),
    ] {
        let answer = app.mfa_confirm(&cookie, &code).await;
        assert_eq!(answer.status, StatusCode::UNPROCESSABLE_ENTITY);
        assert_eq!(
            answer.json(),
            json!({ "error": "invalid", "fields": { "code": { "code": reason } } })
        );
    }
    assert_eq!(app.factor(ann.id).await.unwrap().confirmed_at, None);

    // A right one does, and issues 10 recovery codes, shown this once (AUTH-06).
    let answer = app
        .mfa_confirm(&cookie, &code_at(&secret, step_now()))
        .await;
    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    let codes: Vec<String> = answer.json()["recoveryCodes"]
        .as_array()
        .unwrap()
        .iter()
        .map(|code| code.as_str().unwrap().to_owned())
        .collect();
    assert_eq!(codes.len(), 10);
    for code in &codes {
        assert_eq!(code.len(), 19, "{code}");
        assert_eq!(code.matches('-').count(), 3, "{code}");
    }
    let mut distinct = codes.clone();
    distinct.sort();
    distinct.dedup();
    assert_eq!(distinct.len(), 10);

    // SEC-07: recovery codes are stored only as the SHA-256 of their characters.
    let mut stored: Vec<Vec<u8>> = recovery_code::Entity::find()
        .filter(recovery_code::Column::UserId.eq(ann.id))
        .filter(recovery_code::Column::UsedAt.is_null())
        .all(&app.db)
        .await
        .unwrap()
        .into_iter()
        .map(|row| row.code_hash)
        .collect();
    let mut hashes: Vec<Vec<u8>> = codes
        .iter()
        .map(|code| Sha256::digest(code.replace('-', "")).to_vec())
        .collect();
    stored.sort();
    hashes.sort();
    assert_eq!(stored, hashes);

    let status = app.mfa(&cookie).await.json();
    assert_eq!(status["enabled"], true);
    assert_eq!(status["recoveryCodesLeft"], 10);
    let enabled_at = status["enabledAt"].as_str().unwrap();
    assert!(
        Utc::now() - enabled_at.parse::<chrono::DateTime<Utc>>().unwrap() < TimeDelta::minutes(1)
    );

    let events = app.events(Action::MfaEnabled).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["actor_id"], json!(ann.id));
    assert_eq!(events[0]["target_id"], json!(ann.id));
    assert_eq!(events[0]["outcome"], "success");
    assert_eq!(events[0]["details"], json!({}));

    // Neither setting up again nor confirming again replaces an app that is on.
    let answer = app.mfa_setup(&cookie).await;
    assert_eq!(answer.status, StatusCode::CONFLICT);
    assert_eq!(answer.json(), json!({ "error": "mfa_enabled" }));
    let answer = app
        .mfa_confirm(&cookie, &code_at(&secret, step_now()))
        .await;
    assert_eq!(answer.status, StatusCode::CONFLICT);
    assert_eq!(
        Cipher::new(&mfa_key())
            .decrypt(ann.id, &app.factor(ann.id).await.unwrap().secret_encrypted)
            .unwrap()
            .expose(),
        secret.expose()
    );
}

fn step_now() -> i64 {
    twoviewer_server::accounts::mfa::totp::step(Utc::now())
}

#[tokio::test]
async fn setting_up_again_replaces_an_unconfirmed_app() {
    let app = app(|_| {}).await;
    let ann = app.create_user("ann@example.com").await;
    let (cookie, _) = signed_in(&app, "ann@example.com").await;

    let first = secret_of(&app.mfa_setup(&cookie).await);
    let second = secret_of(&app.mfa_setup(&cookie).await);

    assert_ne!(first.expose(), second.expose());
    let answer = app.mfa_confirm(&cookie, &code_at(&first, step_now())).await;
    assert_eq!(answer.status, StatusCode::UNPROCESSABLE_ENTITY);
    let answer = app
        .mfa_confirm(&cookie, &code_at(&second, step_now()))
        .await;
    assert_eq!(answer.status, StatusCode::OK);
    assert!(app.factor(ann.id).await.unwrap().is_enabled());

    // Without a setup, there is nothing to confirm.
    let (bob_cookie, _) = {
        app.create_user("bob@example.com").await;
        signed_in(&app, "bob@example.com").await
    };
    let answer = app.mfa_confirm(&bob_cookie, "123456").await;
    assert_eq!(answer.status, StatusCode::GONE);
    assert_eq!(answer.json(), json!({ "error": "mfa_setup_expired" }));
}

#[tokio::test]
async fn sign_in_with_mfa_takes_a_code_from_the_app() {
    let app = app(|_| {}).await;
    let ann = app.create_user("ann@example.com").await;
    let (cookie, _) = signed_in(&app, "ann@example.com").await;
    let (secret, _) = app.enable_mfa(&cookie).await;

    // The right password doesn't sign in yet (AUTH-05): no sign-in exists until the second step.
    let answer = app.sign_in("ann@example.com", PASSWORD).await;
    assert_eq!(answer.status, StatusCode::PRECONDITION_REQUIRED);
    assert_eq!(answer.json(), json!({ "error": "second_factor_required" }));
    assert_eq!(app.sessions().await.len(), 1);
    // SEC-09: the pending sign-in's token is hidden from page scripts too.
    let set_cookie = answer.set_cookie().unwrap();
    let (name_value, attributes) = set_cookie.split_once("; ").unwrap();
    assert_eq!(
        attributes,
        "Path=/; Max-Age=300; Secure; HttpOnly; SameSite=Strict"
    );
    let token = name_value
        .strip_prefix("__Host-twoviewer-sign-in=")
        .unwrap();
    let pending = pending_sign_in::Entity::find().all(&app.db).await.unwrap();
    assert_eq!(pending.len(), 1);
    assert_eq!(pending[0].user_id, ann.id);
    assert_eq!(pending[0].token_hash, token::hash(token));
    let pending_cookie = answer.cookie();
    assert!(app.events(Action::SignInSucceeded).await.len() == 1);

    // A wrong code doesn't sign in.
    let answer = app
        .second_factor(Some(&pending_cookie), &wrong_code(&secret))
        .await;
    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    assert_eq!(answer.json(), json!({ "error": "second_factor_failed" }));
    let failures = app.events(Action::SignInFailed).await;
    assert_eq!(failures.len(), 1);
    assert_eq!(failures[0]["actor_id"], Value::Null);
    assert_eq!(failures[0]["target_id"], json!(ann.id));
    assert_eq!(failures[0]["reason_code"], "invalid_second_factor");
    assert_eq!(app.failed_metric(), 1);

    // A right one does, and ends the pending sign-in.
    let code = app.next_code(ann.id, &secret).await;
    let answer = app.second_factor(Some(&pending_cookie), &code).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.body);
    let cookies = set_cookies(&answer);
    assert_eq!(cookies.len(), 2);
    assert!(cookies[0].starts_with("__Host-twoviewer-session="));
    assert_eq!(
        cookies[1],
        "__Host-twoviewer-sign-in=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Strict"
    );
    assert!(app.is_signed_in(&answer.cookie()).await);
    assert!(
        pending_sign_in::Entity::find()
            .all(&app.db)
            .await
            .unwrap()
            .is_empty()
    );
    let session = app.sessions().await.pop().unwrap();
    assert_eq!(session.authenticated_at, session.created_at);
    let succeeded = app.events(Action::SignInSucceeded).await;
    assert_eq!(succeeded.len(), 2);
    assert_eq!(succeeded[1]["actor_id"], json!(ann.id));
    assert_eq!(
        succeeded[1]["details"],
        json!({ "sign_in_session_id": session.id, "second_factor": "authenticator" })
    );

    // The pending sign-in is used once.
    let answer = app.second_factor(Some(&pending_cookie), &code).await;
    assert_eq!(answer.status, StatusCode::GONE);
    assert_eq!(answer.json(), json!({ "error": "sign_in_step_expired" }));
    assert_eq!(
        answer.set_cookie(),
        Some("__Host-twoviewer-sign-in=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Strict")
    );

    // A code is used once too (RFC 6238 §5.2).
    let pending_cookie = password_step(&app, "ann@example.com").await;
    let answer = app.second_factor(Some(&pending_cookie), &code).await;
    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);

    // Without the cookie, or with an empty code.
    let answer = app.second_factor(None, &code).await;
    assert_eq!(answer.status, StatusCode::GONE);
    let answer = app.second_factor(Some(&pending_cookie), " ").await;
    assert_eq!(answer.status, StatusCode::UNPROCESSABLE_ENTITY);
    assert_eq!(
        answer.json(),
        json!({ "error": "invalid", "fields": { "code": { "code": "required" } } })
    );
}

#[tokio::test]
async fn recovery_codes_stand_in_for_the_app_once_each() {
    let app = app(|_| {}).await;
    let ann = app.create_user("ann@example.com").await;
    let (cookie, _) = signed_in(&app, "ann@example.com").await;
    let (_, codes) = app.enable_mfa(&cookie).await;

    // Typed generously: in capitals, with spaces instead of hyphens.
    let typed = codes[3].to_uppercase().replace('-', " ");
    let pending_cookie = password_step(&app, "ann@example.com").await;
    let answer = app.second_factor(Some(&pending_cookie), &typed).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.body);

    let session = app.sessions().await.pop().unwrap();
    let succeeded = app.events(Action::SignInSucceeded).await;
    assert_eq!(
        succeeded.last().unwrap()["details"],
        json!({ "sign_in_session_id": session.id, "second_factor": "recovery" })
    );
    let used = app.events(Action::RecoveryCodeUsed).await;
    assert_eq!(used.len(), 1);
    assert_eq!(used[0]["actor_id"], json!(ann.id));
    assert_eq!(used[0]["details"], json!({ "remaining": 9 }));
    assert_eq!(app.mfa(&cookie).await.json()["recoveryCodesLeft"], 9);

    // Once.
    let pending_cookie = password_step(&app, "ann@example.com").await;
    let answer = app.second_factor(Some(&pending_cookie), &codes[3]).await;
    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    assert_eq!(app.events(Action::RecoveryCodeUsed).await.len(), 1);

    // Another one works.
    let answer = app.second_factor(Some(&pending_cookie), &codes[4]).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    assert_eq!(app.mfa(&cookie).await.json()["recoveryCodesLeft"], 8);
}

#[tokio::test]
async fn wrong_codes_block_the_second_step_without_revealing_the_account() {
    let app = app(|config| config.mfa.max_failures = 3).await;
    let ann = app.create_user("ann@example.com").await;
    let (cookie, _) = signed_in(&app, "ann@example.com").await;
    let (secret, codes) = app.enable_mfa(&cookie).await;
    let pending_cookie = password_step(&app, "ann@example.com").await;

    for _ in 0..3 {
        let answer = app
            .second_factor(Some(&pending_cookie), &wrong_code(&secret))
            .await;
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    }
    let blocked = app.events(Action::SignInBlocked).await;
    assert_eq!(blocked.len(), 1);
    assert_eq!(blocked[0]["reason_code"], "too_many_second_factor_failures");
    assert_eq!(blocked[0]["target_id"], json!(ann.id));
    assert_eq!(blocked[0]["details"], json!({ "blocked_seconds": 900 }));

    // Even right codes are refused, without being used up.
    let code = app.next_code(ann.id, &secret).await;
    for right in [&code, &codes[0]] {
        let answer = app.second_factor(Some(&pending_cookie), right).await;
        assert_eq!(answer.status, StatusCode::TOO_MANY_REQUESTS);
        assert!(answer.retry_after() > 800, "{}", answer.retry_after());
    }
    let failures = app.events(Action::SignInFailed).await;
    assert_eq!(failures.last().unwrap()["reason_code"], "blocked");
    assert_eq!(app.failed_metric(), 5);
    assert_eq!(app.mfa(&cookie).await.json()["recoveryCodesLeft"], 10);

    // SEC-14: the password step doesn't reveal the block, so it reveals nothing about the account
    // to someone without the password.
    let answer = app
        .sign_in("ann@example.com", "violet otter harbor 93")
        .await;
    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    password_step(&app, "ann@example.com").await;

    // Once the block has ended, codes work again.
    throttle::Entity::delete_many()
        .filter(throttle::Column::Scope.eq(throttle::Scope::SecondFactor))
        .exec(&app.db)
        .await
        .unwrap();
    let answer = app.second_factor(Some(&pending_cookie), &code).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.body);
}

#[tokio::test]
async fn the_second_step_waits_only_so_long() {
    let app = app(|_| {}).await;
    let ann = app.create_user("ann@example.com").await;
    let (cookie, _) = signed_in(&app, "ann@example.com").await;
    let (secret, _) = app.enable_mfa(&cookie).await;
    let pending_cookie = password_step(&app, "ann@example.com").await;

    pending_sign_in::Entity::update_many()
        .set(pending_sign_in::ActiveModel {
            expires_at: Set(Utc::now() - TimeDelta::seconds(1)),
            ..Default::default()
        })
        .exec(&app.db)
        .await
        .unwrap();

    let code = app.next_code(ann.id, &secret).await;
    let answer = app.second_factor(Some(&pending_cookie), &code).await;
    assert_eq!(answer.status, StatusCode::GONE);
    assert_eq!(answer.json(), json!({ "error": "sign_in_step_expired" }));
    assert_eq!(app.sessions().await.len(), 1);

    // Expired ones are deleted as new ones are stored.
    password_step(&app, "ann@example.com").await;
    assert_eq!(
        pending_sign_in::Entity::find()
            .all(&app.db)
            .await
            .unwrap()
            .len(),
        1
    );
}

#[tokio::test]
async fn turning_mfa_off_needs_a_recent_authentication_with_a_code() {
    let app = app(|_| {}).await;
    let ann = app.create_user("ann@example.com").await;
    let (cookie, session_id) = signed_in(&app, "ann@example.com").await;
    let (secret, _) = app.enable_mfa(&cookie).await;
    // A sign-in waiting for its second step when MFA goes off.
    let pending_cookie = password_step(&app, "ann@example.com").await;

    app.authenticated_ago(session_id, TimeDelta::minutes(20))
        .await;
    let answer = app.mfa_disable(&cookie).await;
    assert_eq!(answer.status, StatusCode::PRECONDITION_REQUIRED);
    assert_eq!(
        answer.json(),
        json!({ "error": "reauthentication_required", "secondFactor": true })
    );

    let code = app.next_code(ann.id, &secret).await;
    let answer = app.reauthenticate(&cookie, PASSWORD, Some(&code)).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.body);
    let answer = app.mfa_disable(&cookie).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT);

    assert!(app.factor(ann.id).await.is_none());
    assert!(
        recovery_code::Entity::find()
            .all(&app.db)
            .await
            .unwrap()
            .is_empty()
    );
    assert_eq!(
        app.mfa(&cookie).await.json(),
        json!({ "enabled": false, "recoveryCodesLeft": 0 })
    );
    let events = app.events(Action::MfaDisabled).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["actor_id"], json!(ann.id));

    // The pending sign-in can't complete; signing in again needs no code.
    let answer = app.second_factor(Some(&pending_cookie), &code).await;
    assert_eq!(answer.status, StatusCode::GONE);
    assert_eq!(
        app.sign_in("ann@example.com", PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );

    // Turning it off again changes nothing, and records nothing.
    assert_eq!(
        app.mfa_disable(&cookie).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(app.events(Action::MfaDisabled).await.len(), 1);
}

#[tokio::test]
async fn users_change_only_their_own_mfa() {
    let app = app(|_| {}).await;
    let ann = app.create_user("ann@example.com").await;
    let bob = app.create_user("bob@example.com").await;
    let (ann_cookie, _) = signed_in(&app, "ann@example.com").await;
    let (bob_cookie, _) = signed_in(&app, "bob@example.com").await;
    let (secret, codes) = app.enable_mfa(&ann_cookie).await;

    // SEC-11: Bob's requests reach only Bob's settings, whatever he sends.
    assert_eq!(app.mfa(&bob_cookie).await.json()["enabled"], false);
    let answer = app
        .mfa_confirm(&bob_cookie, &code_at(&secret, step_now()))
        .await;
    assert_eq!(answer.status, StatusCode::GONE);
    assert_eq!(
        app.mfa_disable(&bob_cookie).await.status,
        StatusCode::NO_CONTENT
    );
    // Ann's recovery codes are no good to Bob, who has none.
    let answer = app
        .reauthenticate(&bob_cookie, PASSWORD, Some(&codes[0]))
        .await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    assert!(app.events(Action::RecoveryCodeUsed).await.is_empty());

    assert!(app.factor(ann.id).await.unwrap().is_enabled());
    assert_eq!(app.mfa(&ann_cookie).await.json()["recoveryCodesLeft"], 10);
    assert!(app.factor(bob.id).await.is_none());
    assert!(app.events(Action::MfaDisabled).await.is_empty());

    // Requests need a sign-in.
    for answer in [
        app.request(axum::http::Method::GET, "/api/mfa", None, &[])
            .await,
        app.request(axum::http::Method::POST, "/api/mfa/setup", None, &[])
            .await,
        app.request(axum::http::Method::DELETE, "/api/mfa", None, &[])
            .await,
    ] {
        assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
        assert_eq!(answer.json(), json!({ "error": "not_signed_in" }));
    }
}
