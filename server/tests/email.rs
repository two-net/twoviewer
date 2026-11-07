//! Email delivery through a real SMTP server, Mailpit (IF-12): what arrives, retries, and dropping
//! and abandoning emails. Needs Docker.

mod common;

use std::{sync::Arc, time::Duration};

use serde_json::json;
use twoviewer_server::{
    config,
    db::EmailAddress,
    email::{Mailer, TestEmail},
    metrics::Metrics,
};

use common::mailpit::{mailpit, unreachable, wait_until};

fn mailer(config: &config::Email) -> (Mailer, Arc<Metrics>) {
    let metrics = Arc::new(Metrics::new());
    let mailer = Mailer::new(config, None, metrics.clone()).unwrap();
    (mailer, metrics)
}

fn ann() -> EmailAddress {
    EmailAddress::new("Ann@Example.com")
}

/// The counts of sent and failed emails.
fn counts(metrics: &Metrics) -> (u64, u64) {
    (metrics.emails_sent.get(), metrics.emails_failed.get())
}

#[tokio::test]
async fn emails_arrive_with_the_name_logo_and_both_bodies() {
    let mailpit = mailpit().await;
    let (mailer, metrics) = mailer(&mailpit.config());

    mailer.send(&ann(), &TestEmail);

    wait_until("sent", || async { counts(&metrics) == (1, 0) }).await;
    let messages = mailpit.messages().await;
    assert_eq!(messages.len(), 1, "{messages:?}");
    let id = messages[0]["ID"].as_str().unwrap();
    let message = mailpit.api("GET", &format!("message/{id}"), None).await;
    assert_eq!(
        message["From"],
        json!({ "Name": "TwoViewer", "Address": "no-reply@twoviewer.test" })
    );
    assert_eq!(message["To"][0]["Address"], "ann@example.com");
    assert_eq!(message["Subject"], "Test email from TwoViewer");
    let text = message["Text"].as_str().unwrap();
    assert!(text.starts_with("Email delivery works"), "{text}");
    let html = message["HTML"].as_str().unwrap();
    assert!(html.contains(r#"<img src="cid:logo""#), "{html}");
    let inline = message["Inline"].as_array().unwrap();
    assert_eq!(inline.len(), 1, "{inline:?}");
    assert_eq!(inline[0]["ContentType"], "image/png");
    assert_eq!(inline[0]["ContentID"], "logo");
    let message_id = message["MessageID"].as_str().unwrap();
    assert!(message_id.ends_with("@twoviewer.test"), "{message_id}");

    let headers = mailpit
        .api("GET", &format!("message/{id}/headers"), None)
        .await;
    assert_eq!(headers["Auto-Submitted"], json!(["auto-generated"]));
}

#[tokio::test]
async fn send_now_returns_the_reply() {
    let mailpit = mailpit().await;
    let (mailer, _) = mailer(&mailpit.config());

    let response = mailer.send_now(&ann(), &TestEmail).await.unwrap();

    assert!(response.is_positive(), "{response:?}");
    assert_eq!(mailpit.messages().await.len(), 1);

    let (unreachable, _) = self::mailer(&unreachable());
    assert!(unreachable.send_now(&ann(), &TestEmail).await.is_err());
}

#[tokio::test]
async fn temporary_failures_are_tried_again() {
    let mailpit = mailpit().await;
    let mut config = mailpit.config();
    config.max_attempts = 1000;
    config.retry_delay = Duration::from_millis(50);
    let (mailer, metrics) = mailer(&config);
    mailpit.fail_recipients(Some(451)).await;

    mailer.send(&ann(), &TestEmail);

    // Several attempts, all failed for now.
    tokio::time::sleep(Duration::from_millis(500)).await;
    assert_eq!(counts(&metrics), (0, 0));
    assert!(mailpit.messages().await.is_empty());

    mailpit.fail_recipients(None).await;
    wait_until("sent", || async { counts(&metrics) == (1, 0) }).await;
    assert_eq!(mailpit.messages().await.len(), 1);
}

#[tokio::test]
async fn rejected_emails_are_not_tried_again() {
    let mailpit = mailpit().await;
    let mut config = mailpit.config();
    // A second attempt would come only after the test has failed.
    config.retry_delay = Duration::from_secs(60 * 60);
    let (mailer, metrics) = mailer(&config);
    mailpit.fail_recipients(Some(550)).await;

    mailer.send(&ann(), &TestEmail);

    wait_until("failed", || async { counts(&metrics) == (0, 1) }).await;
    assert!(mailpit.messages().await.is_empty());
}

#[tokio::test]
async fn emails_are_given_up_after_the_last_attempt() {
    let mut config = unreachable();
    config.max_attempts = 3;
    config.retry_delay = Duration::from_millis(10);
    let (mailer, metrics) = mailer(&config);

    mailer.send(&ann(), &TestEmail);

    wait_until("failed", || async { counts(&metrics) == (0, 1) }).await;
}

#[tokio::test]
async fn emails_beyond_max_pending_are_dropped_and_shutdown_abandons_waiting_ones() {
    let mut config = unreachable();
    config.max_pending = 1;
    config.retry_delay = Duration::from_secs(60 * 60);
    let (mailer, metrics) = mailer(&config);

    // The first waits to be tried again, which takes the only place.
    mailer.send(&ann(), &TestEmail);
    mailer.send(&ann(), &TestEmail);
    assert_eq!(counts(&metrics), (0, 1));

    tokio::time::timeout(Duration::from_secs(10), mailer.shut_down())
        .await
        .expect("shutdown doesn't wait for retries");
    assert_eq!(counts(&metrics), (0, 2));

    mailer.send(&ann(), &TestEmail);
    assert_eq!(counts(&metrics), (0, 3));
}
