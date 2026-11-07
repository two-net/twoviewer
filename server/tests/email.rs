//! Email delivery through a real SMTP server, Mailpit (IF-12): what arrives, retries, and dropping
//! and abandoning emails. Needs Docker.

mod common;

use std::{
    future::Future,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use serde_json::{Value, json};
use testcontainers_modules::testcontainers::{
    ContainerAsync, GenericImage, ImageExt,
    core::{IntoContainerPort, WaitFor},
    runners::AsyncRunner,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpStream,
};
use twoviewer_server::{
    config::{self, SmtpSecurity},
    db::EmailAddress,
    email::{Mailer, TestEmail},
    metrics::Metrics,
};

use common::server::free_address;

/// The Mailpit release, the same as in `compose.yaml`.
const MAILPIT_TAG: &str = "v1.33.0";

/// A Mailpit server for one test, removed when dropped.
struct Mailpit {
    _container: ContainerAsync<GenericImage>,
    smtp: SocketAddr,
    api: SocketAddr,
}

async fn mailpit() -> Mailpit {
    let container = GenericImage::new("axllent/mailpit", MAILPIT_TAG)
        .with_exposed_port(1025.tcp())
        .with_exposed_port(8025.tcp())
        .with_wait_for(WaitFor::message_on_either_std("[http] starting on"))
        // Chaos makes the SMTP server fail on request.
        .with_cmd(["--enable-chaos", "--disable-version-check"])
        .start()
        .await
        .expect("start a Mailpit container (is Docker running?)");
    let host = container.get_host().await.unwrap().to_string();
    let ipv4 = |port| {
        let host = host.clone();
        async move {
            tokio::net::lookup_host((host, port))
                .await
                .unwrap()
                .find(SocketAddr::is_ipv4)
                .unwrap()
        }
    };
    let smtp = ipv4(container.get_host_port_ipv4(1025).await.unwrap()).await;
    let api = ipv4(container.get_host_port_ipv4(8025).await.unwrap()).await;
    // With Colima, the ports are forwarded only a moment after the container has started.
    wait_until("Mailpit is reachable", || async {
        greets(smtp).await && answers(api).await
    })
    .await;
    Mailpit {
        _container: container,
        smtp,
        api,
    }
}

/// Whether an SMTP server at `address` greets.
async fn greets(address: SocketAddr) -> bool {
    let Ok(mut stream) = TcpStream::connect(address).await else {
        return false;
    };
    let mut greeting = [0; 3];
    stream.read_exact(&mut greeting).await.is_ok() && &greeting == b"220"
}

/// Whether Mailpit's API at `address` answers.
async fn answers(address: SocketAddr) -> bool {
    let Ok(mut stream) = TcpStream::connect(address).await else {
        return false;
    };
    let mut response = String::new();
    stream
        .write_all(b"GET /api/v1/info HTTP/1.0\r\n\r\n")
        .await
        .is_ok()
        && stream.read_to_string(&mut response).await.is_ok()
        && response.starts_with("HTTP/1.0 200")
}

impl Mailpit {
    /// Calls Mailpit's API. HTTP/1.0, so that the response is neither chunked nor kept alive.
    async fn api(&self, method: &str, path: &str, body: Option<Value>) -> Value {
        let body = body.map(|body| body.to_string()).unwrap_or_default();
        let mut stream = TcpStream::connect(self.api).await.unwrap();
        let request = format!(
            "{method} /api/v1/{path} HTTP/1.0\r\nContent-Type: application/json\r\n\
             Content-Length: {}\r\n\r\n{body}",
            body.len()
        );
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        assert!(head.starts_with("HTTP/1.0 200"), "{response}");
        serde_json::from_str(body).unwrap_or(Value::Null)
    }

    /// The summaries of the messages received.
    async fn messages(&self) -> Vec<Value> {
        let messages = self.api("GET", "messages", None).await;
        messages["messages"].as_array().unwrap().clone()
    }

    /// Makes the SMTP server reply `code` to every recipient, or accept them again with `None`.
    async fn fail_recipients(&self, code: Option<u16>) {
        let chaos = json!({
            "Recipient": { "ErrorCode": code.unwrap_or(451), "Probability": u8::from(code.is_some()) * 100 },
        });
        self.api("PUT", "chaos", Some(chaos)).await;
    }

    fn config(&self) -> config::Email {
        config::Email {
            smtp_host: Some(self.smtp.ip().to_string()),
            smtp_port: self.smtp.port(),
            ..unreachable()
        }
    }
}

/// Settings for an SMTP server where nothing listens, so every attempt fails at once.
fn unreachable() -> config::Email {
    config::Email {
        from_address: Some("no-reply@twoviewer.test".to_owned()),
        smtp_host: Some("localhost".to_owned()),
        smtp_port: free_address().port(),
        smtp_security: SmtpSecurity::None,
        timeout: Duration::from_secs(5),
        ..config::Email::default()
    }
}

fn mailer(config: &config::Email) -> (Mailer, Arc<Metrics>) {
    let metrics = Arc::new(Metrics::new());
    let mailer = Mailer::new(config, None, metrics.clone()).unwrap();
    (mailer, metrics)
}

fn ann() -> EmailAddress {
    EmailAddress::new("Ann@Example.com")
}

/// Waits until `condition` holds, for at most 30 seconds.
async fn wait_until<F: Future<Output = bool>>(what: &str, mut condition: impl FnMut() -> F) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !condition().await {
        assert!(Instant::now() < deadline, "never: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
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
