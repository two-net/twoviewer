//! Account settings against a real PostgreSQL and Mailpit (AUTH-11): changing the display name,
//! the email address with a link to the new one, and the password with the current one. The
//! account endpoints run in-process. Needs Docker.

mod accounts;
mod common;

use std::time::Duration;

use axum::http::{Method, StatusCode};
use chrono::{TimeDelta, Utc};
use sea_orm::{
    ActiveModelTrait, ColumnTrait, EntityTrait, IntoActiveModel, QueryFilter, Set, prelude::Uuid,
};
use serde_json::{Value, json};
use twoviewer_server::{
    accounts::{
        email_link::{self, Purpose},
        pending_sign_in, user,
    },
    audit::Action,
    db::EmailAddress,
};

use accounts::{Answer, App, PASSWORD, SAFARI, app, elsewhere};
use common::mailpit::wait_until;

const NEW_PASSWORD: &str = "amber lantern meadow 41";
const WRONG: &str = "violet otter harbor 93";
const CONFIRM_SUBJECT: &str = "Confirm your new email address for TwoViewer";
const IN_USE_SUBJECT: &str = "This email address already has a TwoViewer account";
const CHANGED_SUBJECT: &str = "The email address of your TwoViewer account has changed";

/// An account with `email`, verified, as most accounts are.
async fn verified_user(app: &App, email: &str) -> user::Model {
    let mut user = app.create_user(email).await.into_active_model();
    user.email_verified_at = Set(Some(Utc::now()));
    user.update(&app.db).await.unwrap()
}

/// Signs `email` in, and returns the cookie and the ID of the new sign-in.
async fn signed_in(app: &App, email: &str) -> (String, Uuid) {
    let cookie = app.signed_in(email).await;
    let id = app.sessions().await.last().unwrap().id;
    (cookie, id)
}

async fn user(app: &App, id: Uuid) -> user::Model {
    user::Entity::find_by_id(id)
        .one(&app.db)
        .await
        .unwrap()
        .unwrap()
}

/// The token of the link to `page` in `email`.
fn token(email: &Value, page: &str) -> String {
    let text = email["Text"].as_str().unwrap();
    let prefix = format!("https://twoviewer.test/{page}#");
    let link = text
        .lines()
        .find(|line| line.starts_with(&prefix))
        .unwrap_or_else(|| panic!("no link to {page} in {text}"));
    link.split_once('#').unwrap().1.to_owned()
}

/// Asks to change the address of the account signed in with `cookie` to `email`, and returns the
/// token of the link in the `nth` email to it, counting from 1.
async fn change_link(app: &App, cookie: &str, email: &str, nth: usize) -> String {
    let answer = app.change_email(cookie, email).await;
    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.body);
    let address = EmailAddress::new(email);
    let emails = app.emails(address.as_str(), nth).await;
    assert_eq!(emails[nth - 1]["Subject"], CONFIRM_SUBJECT);
    token(&emails[nth - 1], "confirm-email")
}

/// The links that change an address, oldest first.
async fn change_links(app: &App) -> Vec<email_link::Model> {
    let mut links = email_link::Entity::find()
        .filter(email_link::Column::Purpose.eq(Purpose::ChangeEmail))
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

fn field_errors(answer: &Answer) -> Value {
    assert_eq!(
        answer.status,
        StatusCode::UNPROCESSABLE_ENTITY,
        "{}",
        answer.body
    );
    assert_eq!(answer.json()["error"], "invalid");
    answer.json()["fields"].clone()
}

#[tokio::test]
async fn the_display_name_changes_without_reauthenticating() {
    let app = app(|config| config.accounts.display_name_max_length = 20).await;
    let ann = app.create_user("ann@example.com").await;
    let (cookie, session_id) = signed_in(&app, "ann@example.com").await;
    // The name doesn't let anyone in, so an old sign-in may change it.
    app.authenticated_ago(session_id, TimeDelta::hours(1)).await;

    let answer = app
        .change_display_name(Some(&cookie), "  Ann B. Lee ")
        .await;

    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.body);
    assert_eq!(answer.body, "");
    assert_eq!(
        app.account(Some(&cookie)).await.json()["displayName"],
        "Ann B. Lee"
    );
    let events = app.events(Action::DisplayNameChanged).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["actor_id"], json!(ann.id));
    assert_eq!(events[0]["target_id"], json!(ann.id));
    assert_eq!(events[0]["outcome"], "success");
    // Not the names, which are personal data.
    assert_eq!(events[0]["details"], json!({}));

    // The same name changes nothing, and records nothing.
    let answer = app.change_display_name(Some(&cookie), "Ann B. Lee").await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    assert_eq!(app.events(Action::DisplayNameChanged).await.len(), 1);

    // The rules of sign-up.
    for (name, error) in [
        (" ", json!({ "code": "required" })),
        (
            "Ann Bernadette Lee-Smith",
            json!({ "code": "too_long", "max": 20 }),
        ),
        ("Ann\u{7}Lee", json!({ "code": "invalid" })),
    ] {
        let answer = app.change_display_name(Some(&cookie), name).await;
        assert_eq!(field_errors(&answer), json!({ "displayName": error }));
    }
    assert_eq!(user(&app, ann.id).await.display_name, "Ann B. Lee");
    assert_eq!(app.events(Action::DisplayNameChanged).await.len(), 1);

    let answer = app.change_display_name(None, "Mallory").await;
    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
}

/// AUTH-10: changing the address or the password needs the password within 15 minutes.
#[tokio::test]
async fn changing_the_address_or_password_needs_a_recent_authentication() {
    let app = app(|_| {}).await;
    let ann = verified_user(&app, "ann@example.com").await;
    let (cookie, session_id) = signed_in(&app, "ann@example.com").await;
    app.authenticated_ago(session_id, TimeDelta::minutes(16))
        .await;

    for answer in [
        app.change_email(&cookie, "ann.new@example.com").await,
        app.change_password(&cookie, PASSWORD, NEW_PASSWORD).await,
    ] {
        assert_eq!(answer.status, StatusCode::PRECONDITION_REQUIRED);
        assert_eq!(
            answer.json(),
            json!({ "error": "reauthentication_required", "secondFactor": false })
        );
    }

    assert!(change_links(&app).await.is_empty());
    assert_eq!(user(&app, ann.id).await, ann);
    assert!(app.events(Action::EmailChangeRequested).await.is_empty());
    assert!(app.events(Action::PasswordChanged).await.is_empty());

    assert_eq!(
        app.reauthenticate(&cookie, PASSWORD, None).await.status,
        StatusCode::NO_CONTENT
    );
    change_link(&app, &cookie, "ann.new@example.com", 1).await;
}

#[tokio::test]
async fn the_address_changes_once_the_new_one_is_confirmed() {
    let app = app(|_| {}).await;
    let ann = verified_user(&app, "ann@example.com").await;
    let cookie = app.signed_in("ann@example.com").await;

    let answer = app.change_email(&cookie, " Ann.New@Example.com").await;

    assert_eq!(answer.status, StatusCode::ACCEPTED, "{}", answer.body);
    assert_eq!(answer.body, "");
    let emails = app.emails("ann.new@example.com", 1).await;
    assert_eq!(emails[0]["Subject"], CONFIRM_SUBJECT);
    let text = emails[0]["Text"].as_str().unwrap();
    assert!(
        text.contains("The link works once, for 24 hours."),
        "{text}"
    );
    for body in ["Text", "HTML"] {
        assert!(!emails[0][body].as_str().unwrap().contains("Ann Lee"));
    }
    // The database has only the token's hash (SEC-07), and the new address in lowercase (DB-13).
    let token = token(&emails[0], "confirm-email");
    let links = change_links(&app).await;
    assert_eq!(links.len(), 1);
    assert_eq!(links[0].user_id, ann.id);
    assert_eq!(links[0].token_hash, email_link::hash(&token));
    assert_eq!(
        links[0].new_email,
        Some(EmailAddress::new("ann.new@example.com"))
    );
    assert_eq!(links[0].used_at, None);
    let lifetime = links[0].expires_at - Utc::now();
    assert!(
        (23 * 60 * 60..=24 * 60 * 60).contains(&lifetime.num_seconds()),
        "{lifetime}"
    );
    let requested = app.events(Action::EmailChangeRequested).await;
    assert_eq!(requested.len(), 1);
    assert_eq!(requested[0]["actor_id"], json!(ann.id));
    assert_eq!(requested[0]["target_id"], json!(ann.id));
    assert_eq!(requested[0]["outcome"], "success");
    assert_eq!(requested[0]["details"], json!({}));

    // Nothing changes until the link is used, but the change shows.
    let account = app.account(Some(&cookie)).await.json();
    assert_eq!(account["email"], "ann@example.com");
    assert_eq!(account["pendingEmail"], "ann.new@example.com");
    assert_eq!(
        app.sign_in("ann.new@example.com", PASSWORD).await.status,
        StatusCode::UNAUTHORIZED
    );

    let answer = app.confirm_email(&token).await;

    assert_eq!(answer.status, StatusCode::OK, "{}", answer.body);
    assert_eq!(answer.json(), json!({ "email": "ann.new@example.com" }));
    let account = app.account(Some(&cookie)).await.json();
    assert_eq!(account["email"], "ann.new@example.com");
    assert_eq!(account["emailVerified"], true);
    assert_eq!(account.get("pendingEmail"), None);
    let completed = app.events(Action::EmailChangeCompleted).await;
    assert_eq!(completed.len(), 1);
    assert_eq!(completed[0]["actor_id"], json!(ann.id));
    assert_eq!(completed[0]["target_id"], json!(ann.id));
    assert_eq!(completed[0]["outcome"], "success");
    assert_eq!(completed[0]["details"], json!({}));
    // The address was verified already.
    assert!(app.events(Action::EmailVerified).await.is_empty());

    // The old address is told, with the new one.
    let notices = app.emails("ann@example.com", 1).await;
    assert_eq!(notices[0]["Subject"], CHANGED_SUBJECT);
    for body in ["Text", "HTML"] {
        let body = notices[0][body].as_str().unwrap();
        assert!(body.contains("ann.new@example.com"), "{body}");
        assert!(body.contains("https://twoviewer.test/settings"), "{body}");
    }

    // The sign-in stays, and the new address signs in.
    assert!(app.is_signed_in(&cookie).await);
    assert_eq!(
        app.sign_in("ann.new@example.com", PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(
        app.sign_in("ann@example.com", PASSWORD).await.status,
        StatusCode::UNAUTHORIZED
    );
    // The old address is free again.
    assert!(
        user::Entity::find()
            .filter(user::Column::Email.eq(EmailAddress::new("ann@example.com")))
            .one(&app.db)
            .await
            .unwrap()
            .is_none()
    );

    // Opening the link again, as email scanners do, gives the same answer and changes nothing.
    let again = app.confirm_email(&token).await;
    assert_eq!(again.status, StatusCode::OK);
    assert_eq!(again.json(), json!({ "email": "ann.new@example.com" }));
    assert_eq!(app.events(Action::EmailChangeCompleted).await.len(), 1);
    sent_exactly(&app, 2).await;
}

/// Links went to the old address. Whoever reads its mail must not be able to use them once the
/// address has changed: a reset link would set the password of the account at the new address.
#[tokio::test]
async fn confirming_uses_up_the_links_sent_to_the_old_address() {
    let app = app(|_| {}).await;
    let ann = app.create_user("ann@example.com").await;
    let cookie = app.signed_in("ann@example.com").await;
    let verification = email_link::create(
        &app.db,
        ann.id,
        Purpose::VerifyEmail,
        None,
        Duration::from_secs(60 * 60),
    )
    .await
    .unwrap();
    let reset = app
        .send_json(
            Method::POST,
            "/api/password-reset",
            None,
            json!({ "email": "ann@example.com" }),
        )
        .await;
    assert_eq!(reset.status, StatusCode::ACCEPTED);
    let reset = token(&app.emails("ann@example.com", 1).await[0], "reset-password");
    let change = change_link(&app, &cookie, "ann.new@example.com", 1).await;

    assert_eq!(app.confirm_email(&change).await.status, StatusCode::OK);

    let links = email_link::Entity::find().all(&app.db).await.unwrap();
    assert_eq!(links.len(), 3);
    assert!(links.iter().all(|link| link.used_at.is_some()), "{links:?}");
    let answer = app
        .send_json(
            Method::POST,
            "/api/password-reset/complete",
            None,
            json!({ "token": reset, "password": NEW_PASSWORD }),
        )
        .await;
    assert_eq!(answer.status, StatusCode::GONE);
    // Verifying again changes nothing: the new address is verified.
    let answer = app
        .send_json(
            Method::POST,
            "/api/verify-email",
            None,
            json!({ "token": verification.expose() }),
        )
        .await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    assert_eq!(app.events(Action::EmailVerified).await.len(), 1);

    // The new address is verified by its link, which is recorded, as is the change.
    assert!(user(&app, ann.id).await.is_verified());
    let verified = app.events(Action::EmailVerified).await;
    let completed = app.events(Action::EmailChangeCompleted).await;
    assert_eq!(verified[0]["actor_id"], json!(ann.id));
    assert_eq!(
        verified[0]["correlation_id"],
        completed[0]["correlation_id"]
    );
    // The old address was never verified, so it may be someone else's: it isn't told.
    sent_exactly(&app, 2).await;
    assert_eq!(app.emails("ann@example.com", 1).await.len(), 1);
}

#[tokio::test]
async fn a_newer_request_replaces_the_earlier_link() {
    let app = app(|_| {}).await;
    let ann = verified_user(&app, "ann@example.com").await;
    let cookie = app.signed_in("ann@example.com").await;
    let typo = change_link(&app, &cookie, "ann@exmaple.com", 1).await;

    let fixed = change_link(&app, &cookie, "ann.new@example.com", 1).await;

    assert_eq!(
        app.account(Some(&cookie)).await.json()["pendingEmail"],
        "ann.new@example.com"
    );
    let answer = app.confirm_email(&typo).await;
    assert_eq!(answer.status, StatusCode::GONE);
    assert_eq!(answer.json(), json!({ "error": "link_expired" }));
    assert_eq!(app.confirm_email(&fixed).await.status, StatusCode::OK);
    assert_eq!(
        user(&app, ann.id).await.email.as_str(),
        "ann.new@example.com"
    );
    assert_eq!(app.events(Action::EmailChangeRequested).await.len(), 2);
    assert_eq!(app.events(Action::EmailChangeCompleted).await.len(), 1);
}

/// Being signed in doesn't tell anyone which addresses have an account: the answer, the change
/// waiting, the audit event and the limit are the same, and only the owner of the address learns
/// it.
#[tokio::test]
async fn an_address_with_an_account_looks_the_same_but_gets_no_link() {
    let app = app(|_| {}).await;
    let ann = verified_user(&app, "ann@example.com").await;
    let bob = verified_user(&app, "bob@example.com").await;
    let cookie = app.signed_in("ann@example.com").await;
    let free = app.change_email(&cookie, "carl@example.com").await;
    let pending_free = app.account(Some(&cookie)).await.json()["pendingEmail"].clone();

    let taken = app.change_email(&cookie, "Bob@Example.com").await;

    assert_eq!(taken.status, free.status);
    assert_eq!(taken.body, free.body);
    assert_eq!(pending_free, "carl@example.com");
    assert_eq!(
        app.account(Some(&cookie)).await.json()["pendingEmail"],
        "bob@example.com"
    );
    let emails = app.emails("bob@example.com", 1).await;
    assert_eq!(emails[0]["Subject"], IN_USE_SUBJECT);
    for body in ["Text", "HTML"] {
        let body = emails[0][body].as_str().unwrap();
        assert!(body.contains("https://twoviewer.test/sign-in"), "{body}");
        assert!(
            !body.contains("https://twoviewer.test/confirm-email"),
            "{body}"
        );
    }
    // The link is stored, as for a free address, but never sent.
    let links = change_links(&app).await;
    assert_eq!(links.len(), 2);
    assert!(links[0].used_at.is_some());
    assert_eq!(
        links[1].new_email,
        Some(EmailAddress::new("bob@example.com"))
    );
    assert_eq!(links[1].used_at, None);
    let events = app.events(Action::EmailChangeRequested).await;
    assert_eq!(events.len(), 2);
    for key in ["actor_id", "target_id", "outcome", "reason_code", "details"] {
        assert_eq!(events[0][key], events[1][key], "{key}");
    }
    assert_eq!(user(&app, ann.id).await, ann);
    assert_eq!(user(&app, bob.id).await, bob);
    sent_exactly(&app, 2).await;
}

#[tokio::test]
async fn an_address_that_gets_an_account_meanwhile_cannot_be_confirmed() {
    let app = app(|_| {}).await;
    let ann = verified_user(&app, "ann@example.com").await;
    let cookie = app.signed_in("ann@example.com").await;
    let token = change_link(&app, &cookie, "carl@example.com", 1).await;
    app.create_user("carl@example.com").await;

    let answer = app.confirm_email(&token).await;

    assert_eq!(answer.status, StatusCode::CONFLICT);
    assert_eq!(answer.json(), json!({ "error": "email_in_use" }));
    assert_eq!(user(&app, ann.id).await, ann);
    assert!(app.events(Action::EmailChangeCompleted).await.is_empty());
}

#[tokio::test]
async fn invalid_and_unchanged_addresses_are_rejected() {
    let app = app(|_| {}).await;
    verified_user(&app, "ann@example.com").await;
    let cookie = app.signed_in("ann@example.com").await;

    for (email, code) in [
        (" ", "required"),
        ("not an address", "invalid"),
        (" ANN@example.com", "unchanged"),
    ] {
        let answer = app.change_email(&cookie, email).await;
        assert_eq!(
            field_errors(&answer),
            json!({ "email": { "code": code } }),
            "{email}"
        );
    }

    assert!(change_links(&app).await.is_empty());
    assert!(app.events(Action::EmailChangeRequested).await.is_empty());
    sent_exactly(&app, 0).await;
}

#[tokio::test]
async fn an_account_that_asks_too_often_is_refused() {
    let app = app(|config| config.email_change.max_per_account = 2).await;
    let ann = verified_user(&app, "ann@example.com").await;
    verified_user(&app, "bob@example.com").await;
    let cookie = app.signed_in("ann@example.com").await;
    let bobs = app.signed_in("bob@example.com").await;

    // An address with an account counts too, or the limit would tell which have one.
    for email in ["carl@example.com", "bob@example.com"] {
        assert_eq!(
            app.change_email(&cookie, email).await.status,
            StatusCode::ACCEPTED
        );
    }
    let answer = app.change_email(&cookie, "dave@example.com").await;

    assert_eq!(answer.status, StatusCode::TOO_MANY_REQUESTS);
    assert!(
        (3500..=3600).contains(&answer.retry_after()),
        "{}",
        answer.retry_after()
    );
    let events = app.events(Action::EmailChangeRequested).await;
    assert_eq!(events.len(), 3);
    assert_eq!(events[2]["actor_id"], json!(ann.id));
    assert_eq!(events[2]["outcome"], "failure");
    assert_eq!(events[2]["reason_code"], "too_many_requests");
    // Other accounts are counted on their own.
    assert_eq!(
        app.change_email(&bobs, "erin@example.com").await.status,
        StatusCode::ACCEPTED
    );
    // Two to the new addresses, and Bob's notice.
    sent_exactly(&app, 3).await;
}

#[tokio::test]
async fn expired_and_unknown_links_do_not_change_the_address() {
    let app = app(|_| {}).await;
    let ann = verified_user(&app, "ann@example.com").await;
    let cookie = app.signed_in("ann@example.com").await;
    let token = change_link(&app, &cookie, "ann.new@example.com", 1).await;
    let reset = email_link::create(
        &app.db,
        ann.id,
        Purpose::ResetPassword,
        None,
        Duration::from_secs(60),
    )
    .await
    .unwrap();
    email_link::Entity::update_many()
        .set(email_link::ActiveModel {
            expires_at: Set(Utc::now() - TimeDelta::seconds(1)),
            ..Default::default()
        })
        .filter(email_link::Column::Purpose.eq(Purpose::ChangeEmail))
        .exec(&app.db)
        .await
        .unwrap();

    // The token of a link for something else, and tokens that were never issued.
    for token in [
        token.as_str(),
        reset.expose(),
        "0123",
        &"a".repeat(64),
        &"a".repeat(10_000),
        "",
    ] {
        let answer = app.confirm_email(token).await;
        assert_eq!(answer.status, StatusCode::GONE);
        assert_eq!(answer.json(), json!({ "error": "link_expired" }));
    }

    assert_eq!(user(&app, ann.id).await, ann);
    assert_eq!(
        app.account(Some(&cookie)).await.json().get("pendingEmail"),
        None
    );
}

/// The password changes with the current one, and every other browser is signed out.
#[tokio::test]
async fn changing_the_password_signs_out_everywhere_else() {
    let app = app(|_| {}).await;
    let ann = verified_user(&app, "ann@example.com").await;
    let bob = verified_user(&app, "bob@example.com").await;
    let safari = app
        .sign_in_with("ann@example.com", PASSWORD, elsewhere(), SAFARI)
        .await
        .cookie();
    let bobs = app.signed_in("bob@example.com").await;
    let (cookie, session_id) = signed_in(&app, "ann@example.com").await;

    let answer = app.change_password(&cookie, PASSWORD, NEW_PASSWORD).await;

    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.body);
    assert_eq!(answer.body, "");
    assert!(app.is_signed_in(&cookie).await);
    assert!(!app.is_signed_in(&safari).await);
    assert!(app.is_signed_in(&bobs).await);
    assert_eq!(user(&app, bob.id).await, bob);
    assert_eq!(
        app.sign_in("ann@example.com", PASSWORD).await.status,
        StatusCode::UNAUTHORIZED
    );
    assert_eq!(
        app.sign_in("ann@example.com", NEW_PASSWORD).await.status,
        StatusCode::NO_CONTENT
    );

    let changed = app.events(Action::PasswordChanged).await;
    assert_eq!(changed.len(), 1);
    assert_eq!(changed[0]["actor_id"], json!(ann.id));
    assert_eq!(changed[0]["target_id"], json!(ann.id));
    assert_eq!(changed[0]["outcome"], "success");
    assert_eq!(changed[0]["reason_code"], Value::Null);
    assert_eq!(changed[0]["details"], json!({}));
    let revoked = app.events(Action::SignInSessionRevoked).await;
    assert_eq!(revoked.len(), 1);
    assert_eq!(revoked[0]["actor_id"], json!(ann.id));
    assert_eq!(revoked[0]["correlation_id"], changed[0]["correlation_id"]);
    assert_ne!(
        revoked[0]["details"]["sign_in_session_id"],
        json!(session_id)
    );
}

/// A wrong current password counts toward the block on sign-in with the address (AUTH-08), so a
/// stolen sign-in cookie gives no more guesses.
#[tokio::test]
async fn a_wrong_current_password_counts_toward_the_block() {
    let app = app(|config| config.sign_in.max_failures = 3).await;
    let ann = verified_user(&app, "ann@example.com").await;
    let cookie = app.signed_in("ann@example.com").await;

    for _ in 0..3 {
        let answer = app.change_password(&cookie, WRONG, NEW_PASSWORD).await;
        assert_eq!(
            field_errors(&answer),
            json!({ "currentPassword": { "code": "incorrect" } })
        );
    }

    let failures = app.events(Action::PasswordChanged).await;
    assert_eq!(failures.len(), 3);
    assert_eq!(failures[0]["actor_id"], json!(ann.id));
    assert_eq!(failures[0]["outcome"], "failure");
    assert_eq!(failures[0]["reason_code"], "invalid_credentials");
    assert_eq!(app.events(Action::SignInBlocked).await.len(), 1);
    let emails = app.emails("ann@example.com", 1).await;
    assert_eq!(
        emails[0]["Subject"],
        "Sign-in to your TwoViewer account is paused"
    );

    // Blocked: even the right password is refused, without being checked.
    let answer = app.change_password(&cookie, PASSWORD, NEW_PASSWORD).await;
    assert_eq!(answer.status, StatusCode::TOO_MANY_REQUESTS);
    let failures = app.events(Action::PasswordChanged).await;
    assert_eq!(failures.len(), 4);
    assert_eq!(failures[3]["reason_code"], "blocked");
    assert_eq!(
        app.sign_in("ann@example.com", PASSWORD).await.status,
        StatusCode::TOO_MANY_REQUESTS
    );
    assert_eq!(user(&app, ann.id).await.password_hash, ann.password_hash);
}

#[tokio::test]
async fn new_passwords_follow_the_rules() {
    let app = app(|_| {}).await;
    let ann = verified_user(&app, "ann@example.com").await;
    let cookie = app.signed_in("ann@example.com").await;

    for ((current, new), fields) in [
        (
            ("", ""),
            json!({
                "currentPassword": { "code": "required" },
                "newPassword": { "code": "required" },
            }),
        ),
        (
            (PASSWORD, "short"),
            json!({ "newPassword": { "code": "too_short", "min": 12 } }),
        ),
        (
            (PASSWORD, "password1234"),
            json!({ "newPassword": { "code": "compromised" } }),
        ),
        // The new password is checked first, so a wrong current one isn't counted.
        (
            (WRONG, "short"),
            json!({ "newPassword": { "code": "too_short", "min": 12 } }),
        ),
    ] {
        let answer = app.change_password(&cookie, current, new).await;
        assert_eq!(field_errors(&answer), fields, "{current} {new}");
    }

    assert!(app.events(Action::PasswordChanged).await.is_empty());
    assert_eq!(user(&app, ann.id).await.password_hash, ann.password_hash);
}

/// Sign-ins waiting for a code were started with the old password, so they end.
#[tokio::test]
async fn with_mfa_pending_sign_ins_end_when_the_password_changes() {
    let app = app(|_| {}).await;
    let ann = verified_user(&app, "ann@example.com").await;
    let cookie = app.signed_in("ann@example.com").await;
    let (secret, _) = app.enable_mfa(&cookie).await;
    let pending = app.sign_in("ann@example.com", PASSWORD).await;
    assert_eq!(pending.status, StatusCode::PRECONDITION_REQUIRED);

    let answer = app.change_password(&cookie, PASSWORD, NEW_PASSWORD).await;

    assert_eq!(answer.status, StatusCode::NO_CONTENT, "{}", answer.body);
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
    assert!(app.is_signed_in(&cookie).await);
}
