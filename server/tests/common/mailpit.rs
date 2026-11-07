//! A Mailpit server, which catches the emails the server sends, for tests that look at them.

use std::{
    future::Future,
    net::SocketAddr,
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
use twoviewer_server::config::{self, SmtpSecurity};

use super::server::free_address;

/// The Mailpit release, the same as in `compose.yaml`.
pub const MAILPIT_TAG: &str = "v1.33.0";

/// A Mailpit server for one test, removed when dropped.
pub struct Mailpit {
    _container: ContainerAsync<GenericImage>,
    pub smtp: SocketAddr,
    pub api: SocketAddr,
}

pub async fn mailpit() -> Mailpit {
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
    pub async fn api(&self, method: &str, path: &str, body: Option<Value>) -> Value {
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
    pub async fn messages(&self) -> Vec<Value> {
        let messages = self.api("GET", "messages", None).await;
        messages["messages"].as_array().unwrap().clone()
    }

    /// Makes the SMTP server reply `code` to every recipient, or accept them again with `None`.
    pub async fn fail_recipients(&self, code: Option<u16>) {
        let chaos = json!({
            "Recipient": { "ErrorCode": code.unwrap_or(451), "Probability": u8::from(code.is_some()) * 100 },
        });
        self.api("PUT", "chaos", Some(chaos)).await;
    }

    pub fn config(&self) -> config::Email {
        config::Email {
            smtp_host: Some(self.smtp.ip().to_string()),
            smtp_port: self.smtp.port(),
            ..unreachable()
        }
    }
}

/// Settings for an SMTP server where nothing listens, so every attempt fails at once.
pub fn unreachable() -> config::Email {
    config::Email {
        from_address: Some("no-reply@twoviewer.test".to_owned()),
        smtp_host: Some("localhost".to_owned()),
        smtp_port: free_address().port(),
        smtp_security: SmtpSecurity::None,
        timeout: Duration::from_secs(5),
        ..config::Email::default()
    }
}

/// Waits until `condition` holds, for at most 30 seconds.
pub async fn wait_until<F: Future<Output = bool>>(what: &str, mut condition: impl FnMut() -> F) {
    let deadline = Instant::now() + Duration::from_secs(30);
    while !condition().await {
        assert!(Instant::now() < deadline, "never: {what}");
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

impl Mailpit {
    /// The message with the ID `id`, with its bodies.
    pub async fn message(&self, id: &str) -> Value {
        self.api("GET", &format!("message/{id}"), None).await
    }

    /// The messages sent to `address`, oldest first, with their bodies.
    pub async fn messages_to(&self, address: &str) -> Vec<Value> {
        let mut messages = Vec::new();
        for summary in self.messages().await.iter().rev() {
            let to = summary["To"].as_array().unwrap();
            if to.iter().any(|to| to["Address"] == address) {
                messages.push(self.message(summary["ID"].as_str().unwrap()).await);
            }
        }
        messages
    }
}
