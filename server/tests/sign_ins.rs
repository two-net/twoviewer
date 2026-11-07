//! Sign-ins against a real PostgreSQL (AUTH-09, SEC-09, SEC-11): expiry after inactivity and at
//! most a month after sign-in, the list of a user's sign-ins, revoking them, signing out, and
//! refusing requests that other sites start. The account endpoints run in-process. Needs Docker.

mod accounts;
mod common;

use axum::http::{Method, StatusCode};
use chrono::{TimeDelta, Utc};
use sea_orm::{ActiveModelTrait, IntoActiveModel, Set, prelude::Uuid};
use serde_json::{Value, json};
use twoviewer_server::{
    accounts::sign_in_session::{self, Lifetime},
    audit::Action,
    config,
};

use accounts::{App, HOST, PASSWORD, SAFARI, app, elsewhere};

const CLEAR_COOKIE: &str =
    "__Host-twoviewer-session=; Path=/; Max-Age=0; Secure; HttpOnly; SameSite=Strict";

/// Sets when the sign-in `id` started and was last used, and so when it ends.
async fn set_times(app: &App, id: Uuid, created_at: TimeDelta, last_active_at: TimeDelta) {
    let now = Utc::now();
    let (created_at, last_active_at) = (now - created_at, now - last_active_at);
    let lifetime = Lifetime::new(&config::SignIn::default());
    let mut session = app.session(id).await.into_active_model();
    session.created_at = Set(created_at);
    session.last_active_at = Set(last_active_at);
    session.expires_at = Set(lifetime.expiry(created_at, last_active_at));
    session.update(&app.db).await.unwrap();
}

/// The ID of the newest sign-in.
async fn newest(app: &App) -> Uuid {
    app.sessions().await.pop().unwrap().id
}

/// The sign-in `session` as its user sees it.
fn view(session: &sign_in_session::Model, current: bool) -> Value {
    json!({
        "id": session.id,
        "client": session.client_description,
        "ipAddress": session.ip_address.ip().to_string(),
        "signedInAt": session.created_at,
        "lastActiveAt": session.last_active_at,
        "current": current,
    })
}

#[tokio::test]
async fn sign_ins_end_after_a_week_unused_and_a_month_after_sign_in() {
    let app = app(|_| {}).await;
    app.create_user("ann@example.com").await;
    let cookie = app.signed_in("ann@example.com").await;
    let id = newest(&app).await;
    let signed_in = app.session(id).await;

    // Used again within a minute: nothing is written.
    assert!(app.is_signed_in(&cookie).await);
    assert_eq!(app.session(id).await, signed_in);

    // Used again later: it lasts another week from now.
    set_times(&app, id, TimeDelta::days(3), TimeDelta::days(3)).await;
    let before = Utc::now();
    assert!(app.is_signed_in(&cookie).await);
    let used = app.session(id).await;
    assert!(used.last_active_at >= before - TimeDelta::milliseconds(1));
    assert_eq!(used.expires_at, used.last_active_at + TimeDelta::days(7));

    // But never more than 30 days after sign-in.
    set_times(&app, id, TimeDelta::days(29), TimeDelta::hours(1)).await;
    assert!(app.is_signed_in(&cookie).await);
    let used = app.session(id).await;
    assert_eq!(used.expires_at, used.created_at + TimeDelta::days(30));

    // A week unused ends it, and a later request doesn't bring it back.
    set_times(&app, id, TimeDelta::days(10), TimeDelta::days(8)).await;
    let unused = app.session(id).await;
    assert!(!app.is_signed_in(&cookie).await);
    assert_eq!(app.session(id).await, unused);

    // So do 30 days, however recently it was used.
    set_times(&app, id, TimeDelta::days(31), TimeDelta::minutes(2)).await;
    assert!(!app.is_signed_in(&cookie).await);
}

#[tokio::test]
async fn users_see_their_own_active_sign_ins() {
    let app = app(|_| {}).await;
    app.create_user("ann@example.com").await;
    app.create_user("bob@example.com").await;
    let phone = app
        .sign_in_with("ann@example.com", PASSWORD, elsewhere(), SAFARI)
        .await
        .cookie();
    let phone_id = newest(&app).await;
    app.signed_in("ann@example.com").await;
    let tablet_id = newest(&app).await;
    app.signed_in("bob@example.com").await;
    app.signed_in("ann@example.com").await;
    let revoked_id = newest(&app).await;
    app.signed_in("ann@example.com").await;
    let expired_id = newest(&app).await;
    let laptop = app.signed_in("ann@example.com").await;
    let laptop_id = newest(&app).await;

    set_times(&app, phone_id, TimeDelta::days(3), TimeDelta::hours(1)).await;
    set_times(&app, tablet_id, TimeDelta::days(5), TimeDelta::days(2)).await;
    set_times(&app, expired_id, TimeDelta::days(9), TimeDelta::days(8)).await;
    let mut revoked = app.session(revoked_id).await.into_active_model();
    revoked.revoked_at = Set(Some(Utc::now()));
    revoked.update(&app.db).await.unwrap();

    let answer = app.sign_ins(Some(&laptop)).await;

    // The one of the request first, then the most recently used first.
    assert_eq!(answer.status, StatusCode::OK);
    assert_eq!(
        answer.json(),
        json!({
            "signIns": [
                view(&app.session(laptop_id).await, true),
                view(&app.session(phone_id).await, false),
                view(&app.session(tablet_id).await, false),
            ],
        })
    );
    let phone_view = &answer.json()["signIns"][1];
    assert_eq!(phone_view["client"], "Safari on iOS");
    assert_eq!(phone_view["ipAddress"], "198.51.100.20");

    // From the phone, the phone comes first.
    let answer = app.sign_ins(Some(&phone)).await;
    assert_eq!(answer.json()["signIns"][0]["id"], json!(phone_id));
    assert_eq!(answer.json()["signIns"][0]["current"], true);

    let answer = app.sign_ins(None).await;
    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    assert_eq!(answer.json(), json!({ "error": "not_signed_in" }));
}

#[tokio::test]
async fn revoking_a_sign_in_signs_its_browser_out() {
    let app = app(|_| {}).await;
    let ann = app.create_user("ann@example.com").await;
    let laptop = app.signed_in("ann@example.com").await;
    let laptop_id = newest(&app).await;
    let phone = app.signed_in("ann@example.com").await;
    let phone_id = newest(&app).await;

    let answer = app.revoke(&laptop, phone_id).await;

    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    // The laptop keeps its sign-in, and its cookie.
    assert_eq!(answer.set_cookie(), None);
    assert!(app.is_signed_in(&laptop).await);
    assert!(!app.is_signed_in(&phone).await);
    assert!(app.session(phone_id).await.revoked_at.is_some());

    // Table 3.8-A.
    let events = app.events(Action::SignInSessionRevoked).await;
    assert_eq!(events.len(), 1);
    let event = &events[0];
    assert_eq!(event["actor_type"], "user");
    assert_eq!(event["actor_id"], json!(ann.id));
    assert_eq!(event["target_type"], "user");
    assert_eq!(event["target_id"], json!(ann.id));
    assert_eq!(event["outcome"], "success");
    assert_eq!(event["ip_address"], "203.0.113.7");
    assert_eq!(event["client_description"], "Firefox on Linux");
    assert_eq!(event["details"], json!({ "sign_in_session_id": phone_id }));

    // Once is enough.
    let again = app.revoke(&laptop, phone_id).await;
    assert_eq!(again.status, StatusCode::GONE);
    assert_eq!(again.json(), json!({ "error": "sign_in_ended" }));

    // Revoking the sign-in of the request also removes its cookie.
    let answer = app.revoke(&laptop, laptop_id).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    assert_eq!(answer.set_cookie(), Some(CLEAR_COOKIE));
    assert!(!app.is_signed_in(&laptop).await);
    assert_eq!(app.events(Action::SignInSessionRevoked).await.len(), 2);
}

#[tokio::test]
async fn users_cannot_revoke_the_sign_ins_of_others() {
    let app = app(|_| {}).await;
    app.create_user("ann@example.com").await;
    app.create_user("bob@example.com").await;
    let ann = app.signed_in("ann@example.com").await;
    let bob = app.signed_in("bob@example.com").await;
    let bob_id = newest(&app).await;

    // The same answer as for a sign-in that has ended, so that Ann can't tell (AUTH-17, SEC-11).
    let answer = app.revoke(&ann, bob_id).await;
    assert_eq!(answer.status, StatusCode::GONE);
    assert_eq!(answer.json(), json!({ "error": "sign_in_ended" }));
    let answer = app.revoke(&ann, Uuid::new_v4()).await;
    assert_eq!(answer.status, StatusCode::GONE);

    let answer = app.revoke_others(&ann).await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT);

    assert!(app.is_signed_in(&bob).await);
    assert!(app.events(Action::SignInSessionRevoked).await.is_empty());

    // Not an ID.
    let answer = app.revoke(&ann, "not-an-id").await;
    assert_eq!(answer.status, StatusCode::BAD_REQUEST);
    assert_eq!(answer.json(), json!({ "error": "malformed" }));
    // Without a sign-in.
    let answer = app
        .request(
            Method::DELETE,
            &format!("/api/sign-ins/{bob_id}"),
            None,
            &[],
        )
        .await;
    assert_eq!(answer.status, StatusCode::UNAUTHORIZED);
    assert!(app.is_signed_in(&bob).await);
}

#[tokio::test]
async fn signing_out_everywhere_else_keeps_this_browser_signed_in() {
    let app = app(|_| {}).await;
    let ann = app.create_user("ann@example.com").await;
    app.create_user("bob@example.com").await;
    let laptop = app.signed_in("ann@example.com").await;
    let phone = app.signed_in("ann@example.com").await;
    let phone_id = newest(&app).await;
    let tablet = app.signed_in("ann@example.com").await;
    let tablet_id = newest(&app).await;
    let bob = app.signed_in("bob@example.com").await;

    let answer = app.revoke_others(&laptop).await;

    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    assert_eq!(answer.set_cookie(), None);
    assert!(app.is_signed_in(&laptop).await);
    assert!(!app.is_signed_in(&phone).await);
    assert!(!app.is_signed_in(&tablet).await);
    assert!(app.is_signed_in(&bob).await);
    // One event per sign-in.
    let mut revoked: Vec<_> = app
        .events(Action::SignInSessionRevoked)
        .await
        .iter()
        .map(|event| {
            assert_eq!(event["actor_id"], json!(ann.id));
            event["details"]["sign_in_session_id"].clone()
        })
        .collect();
    revoked.sort_by_key(ToString::to_string);
    let mut expected = vec![json!(phone_id), json!(tablet_id)];
    expected.sort_by_key(ToString::to_string);
    assert_eq!(revoked, expected);

    // With nothing left to revoke.
    assert_eq!(
        app.revoke_others(&laptop).await.status,
        StatusCode::NO_CONTENT
    );
    assert_eq!(app.events(Action::SignInSessionRevoked).await.len(), 2);
}

#[tokio::test]
async fn signing_out_ends_the_sign_in_and_removes_its_cookie() {
    let app = app(|_| {}).await;
    let ann = app.create_user("ann@example.com").await;
    let cookie = app.signed_in("ann@example.com").await;
    let id = newest(&app).await;
    let other = app.signed_in("ann@example.com").await;

    let answer = app.sign_out(Some(&cookie)).await;

    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    assert_eq!(answer.body, "");
    assert_eq!(answer.set_cookie(), Some(CLEAR_COOKIE));
    assert!(!app.is_signed_in(&cookie).await);
    assert!(app.is_signed_in(&other).await);
    assert!(app.session(id).await.revoked_at.is_some());
    // Table 3.8-A.
    let events = app.events(Action::SignOut).await;
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["actor_id"], json!(ann.id));
    assert_eq!(events[0]["target_id"], json!(ann.id));
    assert_eq!(events[0]["outcome"], "success");
    assert_eq!(events[0]["details"], json!({ "sign_in_session_id": id }));
    assert!(app.events(Action::SignInSessionRevoked).await.is_empty());

    // A browser whose sign-in has already ended loses its cookie too, without another event.
    for cookie in [Some(cookie.as_str()), None] {
        let answer = app.sign_out(cookie).await;
        assert_eq!(answer.status, StatusCode::NO_CONTENT, "{cookie:?}");
        assert_eq!(answer.set_cookie(), Some(CLEAR_COOKIE), "{cookie:?}");
    }
    assert_eq!(app.events(Action::SignOut).await.len(), 1);
}

#[tokio::test]
async fn other_sites_cannot_sign_users_in_or_out() {
    let app = app(|_| {}).await;
    app.create_user("ann@example.com").await;
    let laptop = app.signed_in("ann@example.com").await;
    let phone = app.signed_in("ann@example.com").await;
    let phone_id = newest(&app).await;
    let refused = json!({ "error": "cross_site_request" });

    for headers in [
        &[("host", HOST), ("origin", "https://evil.example")][..],
        &[("host", HOST), ("sec-fetch-site", "cross-site")],
        // Another subdomain counts as the same site for the cookie, but not as this origin.
        &[
            ("host", HOST),
            ("origin", "https://status.twoviewer.test"),
            ("sec-fetch-site", "same-site"),
        ],
    ] {
        for (method, uri) in [
            (Method::POST, "/api/sign-out".to_owned()),
            (Method::DELETE, "/api/sign-ins".to_owned()),
            (Method::DELETE, format!("/api/sign-ins/{phone_id}")),
        ] {
            let answer = app.request(method, &uri, Some(&laptop), headers).await;
            assert_eq!(answer.status, StatusCode::FORBIDDEN, "{uri} {headers:?}");
            assert_eq!(answer.json(), refused, "{uri} {headers:?}");
        }
        // Nor sign in with someone else's account in the user's browser.
        let answer = app
            .request(Method::POST, "/api/sign-in", None, headers)
            .await;
        assert_eq!(answer.status, StatusCode::FORBIDDEN, "{headers:?}");
    }
    assert!(app.is_signed_in(&laptop).await);
    assert!(app.is_signed_in(&phone).await);
    assert!(app.events(Action::SignOut).await.is_empty());
    assert!(app.events(Action::SignInSessionRevoked).await.is_empty());

    // The web client's own requests.
    let answer = app
        .request(
            Method::DELETE,
            &format!("/api/sign-ins/{phone_id}"),
            Some(&laptop),
            &[
                ("host", HOST),
                ("origin", "https://twoviewer.test"),
                ("sec-fetch-site", "same-origin"),
            ],
        )
        .await;
    assert_eq!(answer.status, StatusCode::NO_CONTENT);
    assert!(!app.is_signed_in(&phone).await);
}
