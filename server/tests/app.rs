//! The server lifecycle against a real PostgreSQL: readiness tied to migrations and the database
//! (MNT-02, DB-05), and stopping without serving when migrations fail (DB-06). Needs Docker.

mod common;

use std::time::{Duration, Instant};

use common::{execute, postgres, server::start};
use sqlx::{Connection, PgConnection};
use twoviewer_server::{
    app::RunError,
    db::migrate::{LOCK_KEY, MigrationError},
};

/// Waits until a session waits for an advisory lock.
async fn wait_for_lock_waiter(conn: &mut PgConnection) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT FROM pg_locks WHERE locktype = 'advisory' AND NOT granted)",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        if waiting {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the server never waited for the lock"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn ready_only_once_migrations_have_completed() {
    let (_postgres, url) = postgres().await;
    // Another instance is migrating: it holds the migration lock (DB-07).
    let mut other = PgConnection::connect(&url).await.unwrap();
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(LOCK_KEY)
        .execute(&mut other)
        .await
        .unwrap();

    let server = start(&url, |_| {}).await;
    wait_for_lock_waiter(&mut other).await;

    // Alive while waiting for migrations, but not ready (DB-05).
    assert_eq!(server.get("/livez").await, (200, "alive".to_owned()));
    assert_eq!(server.get("/readyz").await, (503, "not ready".to_owned()));

    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(LOCK_KEY)
        .execute(&mut other)
        .await
        .unwrap();
    server.wait_for("/readyz", 200).await;

    // MNT-04: the migration duration includes the wait for the other instance.
    let (status, metrics) = server.get("/metrics").await;
    assert_eq!(status, 200);
    let seconds: f64 = metrics
        .lines()
        .find_map(|line| line.strip_prefix("twoviewer_migration_duration_seconds "))
        .expect(&metrics)
        .parse()
        .unwrap();
    assert!(seconds > 0.0, "{metrics}");

    server.shut_down().await.unwrap();
}

#[tokio::test]
async fn not_ready_while_the_database_is_unreachable() {
    let (postgres, url) = postgres().await;
    let server = start(&url, |_| {}).await;
    server.wait_for("/readyz", 200).await;

    postgres.stop_with_timeout(Some(0)).await.unwrap();

    server.wait_for("/readyz", 503).await;
    assert_eq!(server.get("/livez").await.0, 200);
    server.shut_down().await.unwrap();
}

#[tokio::test]
async fn failed_migrations_stop_the_server() {
    let (_postgres, url) = postgres().await;
    // Not created by migrations, so the server refuses to migrate it.
    execute(&url, "CREATE TABLE stray (id integer)").await;

    let server = start(&url, |_| {}).await;

    let result = tokio::time::timeout(Duration::from_secs(30), server.task)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            result,
            Err(RunError::Migration(MigrationError::NotEmptyWithoutHistory))
        ),
        "{result:?}"
    );
}

/// The account endpoints are served, with the client's address, which audit events need.
#[tokio::test]
async fn the_api_is_served() {
    let (_postgres, url) = postgres().await;
    let server = start(&url, |_| {}).await;
    server.wait_for("/readyz", 200).await;

    let (status, body) = server.post_json("/api/sign-up", r#"{"email":""}"#).await;
    assert_eq!((status, body.as_str()), (400, r#"{"error":"malformed"}"#));

    let invalid = r#"{"email":"ann","displayName":"Ann","password":"short",
                      "termsVersion":"","privacyVersion":""}"#;
    let (status, body) = server.post_json("/api/sign-up", invalid).await;
    assert_eq!(status, 422, "{body}");
    let body: serde_json::Value = serde_json::from_str(&body).unwrap();
    assert_eq!(body["error"], "invalid");

    // Valid apart from the token, so the request gets as far as the database.
    let (status, body) = server
        .post_json("/api/verify-email", r#"{"token":"0123"}"#)
        .await;
    assert_eq!(
        (status, body.as_str()),
        (410, r#"{"error":"link_expired"}"#)
    );

    server.shut_down().await.unwrap();
}

#[tokio::test]
async fn shutdown_while_waiting_for_migrations_stops_promptly() {
    let (_postgres, url) = postgres().await;
    let mut other = PgConnection::connect(&url).await.unwrap();
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(LOCK_KEY)
        .execute(&mut other)
        .await
        .unwrap();
    let server = start(&url, |_| {}).await;
    wait_for_lock_waiter(&mut other).await;

    server.shut_down().await.unwrap();
}
