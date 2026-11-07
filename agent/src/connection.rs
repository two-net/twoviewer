//! The connection to the server (IF-09), which the agent keeps open for as long as it runs. It is
//! outbound only, over HTTPS (IF-08, DEV-10), and reconnects after every interruption with
//! randomized, increasing delays (REL-05). The messages are in the `twoviewer-protocol` crate.

use std::{sync::Arc, time::Duration};

use futures_util::{SinkExt, StreamExt};
use rustls::{
    ClientConfig,
    crypto::ring,
    pki_types::{InvalidDnsNameError, ServerName},
};
use rustls_platform_verifier::BuilderVerifierExt;
use tokio::{
    net::TcpStream,
    sync::{mpsc, watch},
};
use tokio_rustls::{TlsConnector, client::TlsStream};
use tokio_tungstenite::{
    WebSocketStream,
    tungstenite::{self, Message, http::Uri},
};
use tracing::{info, warn};
use twoviewer_protocol::{AGENT_PATH, ClientMessage, Event, ServerMessage};

use crate::backoff;

/// How long opening a connection may take, from the TCP connection to the server's hello.
pub const CONNECT_TIMEOUT: Duration = Duration::from_secs(10);

/// The state of the connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum State {
    /// Opening a connection.
    Connecting,
    /// The server said hello.
    Connected,
    /// Waiting before the next attempt.
    Disconnected,
}

/// What the agent learns from the server.
#[derive(Clone, Debug, PartialEq)]
pub enum Received {
    /// Something changed: fetch it from the server.
    Event(Event),
    /// Events may have been missed: fetch everything. Sent after every connection, too.
    Resync,
}

#[derive(Debug, thiserror::Error)]
pub enum InvalidServer {
    #[error(
        "the server address must be an https:// address without a path, such as \
         https://twoviewer.example"
    )]
    Address,
    #[error("the server's host name is invalid: {0}")]
    HostName(#[from] InvalidDnsNameError),
}

/// TLS 1.2 and 1.3 (IF-08), trusting the certificate authorities that the operating system
/// trusts. That includes those that organizations install for their proxies.
pub fn system_tls() -> Result<Arc<ClientConfig>, rustls::Error> {
    Ok(Arc::new(
        ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
            .with_safe_default_protocol_versions()?
            .with_platform_verifier()?
            .with_no_client_auth(),
    ))
}

/// Why a connection ended, or could not be opened.
#[derive(Debug, thiserror::Error)]
enum Lost {
    #[error("cannot connect: {0}")]
    Io(#[from] std::io::Error),
    #[error("{0}")]
    WebSocket(Box<tungstenite::Error>),
    #[error("no answer within {CONNECT_TIMEOUT:?}")]
    ConnectTimeout,
    #[error("the server did not say hello")]
    NoHello,
    #[error("the server was silent for too long")]
    Silent,
    #[error("closed by the server, code {0:?}")]
    Closed(Option<u16>),
}

impl From<tungstenite::Error> for Lost {
    fn from(err: tungstenite::Error) -> Self {
        Self::WebSocket(Box::new(err))
    }
}

type Socket = WebSocketStream<TlsStream<TcpStream>>;

/// The connection to one server.
pub struct Connection {
    host: String,
    port: u16,
    name: ServerName<'static>,
    url: String,
    tls: TlsConnector,
}

impl Connection {
    /// A connection to `server`, such as `https://twoviewer.example`, verified with `tls`.
    pub fn new(server: &str, tls: Arc<ClientConfig>) -> Result<Self, InvalidServer> {
        let uri: Uri = server.parse().map_err(|_| InvalidServer::Address)?;
        let authority = uri.authority().ok_or(InvalidServer::Address)?;
        if uri.scheme_str() != Some("https")
            || !matches!(uri.path(), "" | "/")
            || uri.query().is_some()
            || authority.as_str().contains('@')
        {
            return Err(InvalidServer::Address);
        }
        let host = authority
            .host()
            .trim_start_matches('[')
            .trim_end_matches(']');
        Ok(Self {
            host: host.to_owned(),
            port: authority.port_u16().unwrap_or(443),
            name: ServerName::try_from(host.to_owned())?,
            url: format!("wss://{authority}{AGENT_PATH}"),
            tls: TlsConnector::from(tls),
        })
    }

    /// Keeps a connection open, reconnecting after every interruption, until dropped. The
    /// connection's `state` goes to `state`, and what the server says to `received`, which must
    /// be read.
    pub async fn run(self, received: mpsc::UnboundedSender<Received>, state: watch::Sender<State>) {
        let mut attempt = 0;
        loop {
            state.send_replace(State::Connecting);
            let lost = self.serve(&received, &state, &mut attempt).await;
            state.send_replace(State::Disconnected);
            let delay = backoff::delay(attempt, rand::random());
            attempt = attempt.saturating_add(1);
            warn!(
                reason = %lost,
                retry_in_ms = u64::try_from(delay.as_millis()).unwrap_or(u64::MAX),
                "not connected to the server"
            );
            tokio::time::sleep(delay).await;
        }
    }

    /// Opens a connection and serves it until it ends. A successful connection resets `attempt`.
    async fn serve(
        &self,
        received: &mpsc::UnboundedSender<Received>,
        state: &watch::Sender<State>,
        attempt: &mut u32,
    ) -> Lost {
        let (mut socket, timeout) =
            match tokio::time::timeout(CONNECT_TIMEOUT, self.connect()).await {
                Ok(Ok(connected)) => connected,
                Ok(Err(lost)) => return lost,
                Err(_) => return Lost::ConnectTimeout,
            };
        *attempt = 0;
        state.send_replace(State::Connected);
        info!(server = %self.url, "connected to the server");
        let _ = received.send(Received::Resync);

        loop {
            let message = match tokio::time::timeout(timeout, socket.next()).await {
                Err(_) => return Lost::Silent,
                Ok(None) => return Lost::Closed(None),
                Ok(Some(Err(err))) => return err.into(),
                Ok(Some(Ok(message))) => message,
            };
            let text = match message {
                Message::Text(text) => text,
                Message::Close(frame) => return Lost::Closed(frame.map(|frame| frame.code.into())),
                // The WebSocket library answers pings itself.
                _ => continue,
            };
            // Messages of a newer server, which this agent does not know, are ignored.
            match serde_json::from_str(&text) {
                Ok(ServerMessage::Heartbeat) => {
                    let answer = serde_json::to_string(&ClientMessage::Heartbeat)
                        .expect("a client message is valid JSON");
                    match tokio::time::timeout(timeout, socket.send(Message::text(answer))).await {
                        Ok(Ok(())) => {}
                        Ok(Err(err)) => return err.into(),
                        Err(_) => return Lost::Silent,
                    }
                }
                Ok(ServerMessage::Event(event)) => {
                    let _ = received.send(Received::Event(event));
                }
                Ok(ServerMessage::Resync) => {
                    let _ = received.send(Received::Resync);
                }
                Ok(ServerMessage::Hello { .. } | ServerMessage::Unknown) | Err(_) => {}
            }
        }
    }

    /// Opens a connection and waits for the server's hello. Returns the connection and how long
    /// the server may be silent.
    async fn connect(&self) -> Result<(Socket, Duration), Lost> {
        let tcp = TcpStream::connect((self.host.as_str(), self.port)).await?;
        let tls = self.tls.connect(self.name.clone(), tcp).await?;
        let (mut socket, _) = tokio_tungstenite::client_async(self.url.as_str(), tls).await?;
        match socket.next().await {
            Some(Ok(Message::Text(text))) => match serde_json::from_str(&text) {
                Ok(ServerMessage::Hello {
                    heartbeat_timeout_ms,
                }) => Ok((socket, Duration::from_millis(heartbeat_timeout_ms))),
                _ => Err(Lost::NoHello),
            },
            Some(Ok(Message::Close(frame))) => {
                Err(Lost::Closed(frame.map(|frame| frame.code.into())))
            }
            Some(Err(err)) => Err(err.into()),
            Some(Ok(_)) | None => Err(Lost::NoHello),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn tls() -> Arc<ClientConfig> {
        Arc::new(
            ClientConfig::builder_with_provider(Arc::new(ring::default_provider()))
                .with_safe_default_protocol_versions()
                .unwrap()
                .with_root_certificates(rustls::RootCertStore::empty())
                .with_no_client_auth(),
        )
    }

    #[test]
    fn the_server_address_gives_the_connection_address() {
        for (server, host, port, url) in [
            (
                "https://twoviewer.example",
                "twoviewer.example",
                443,
                "wss://twoviewer.example/api/agent/realtime",
            ),
            (
                "https://localhost:8443/",
                "localhost",
                8443,
                "wss://localhost:8443/api/agent/realtime",
            ),
            (
                "https://[::1]:8443",
                "::1",
                8443,
                "wss://[::1]:8443/api/agent/realtime",
            ),
        ] {
            let connection = Connection::new(server, tls()).unwrap();

            assert_eq!(connection.host, host, "{server}");
            assert_eq!(connection.port, port, "{server}");
            assert_eq!(connection.url, url, "{server}");
        }
    }

    #[test]
    fn only_https_addresses_without_a_path_are_servers() {
        for server in [
            "http://twoviewer.example",
            "wss://twoviewer.example",
            "twoviewer.example",
            "https://twoviewer.example/api",
            "https://twoviewer.example?x=1",
            "https://user@twoviewer.example",
            "",
        ] {
            assert!(Connection::new(server, tls()).is_err(), "{server}");
        }
    }
}
