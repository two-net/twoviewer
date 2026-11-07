//! The real-time connection of browsers and Host Agents (IF-09): presence, session setup and
//! notifications, so that screens update without a reload (UI-02).
//!
//! The messages are in the `twoviewer-protocol` crate. Browsers connect on
//! [`BROWSER_PATH`](twoviewer_protocol::BROWSER_PATH) and Host Agents on
//! [`AGENT_PATH`](twoviewer_protocol::AGENT_PATH), both WebSockets on the HTTPS listener.
//!
//! Features send events with [`publish`], to a [`Topic`]. Every event goes through PostgreSQL
//! (`NOTIFY`), so it reaches the connections on every instance of the server (CAP-03) without
//! another backing service (CON-02). Publish in the transaction of the change, so that the event
//! is sent only once the change is committed:
//!
//! ```ignore
//! let txn = db.begin().await?;
//! device.update(&txn).await?;
//! realtime::publish(&txn, Topic::Browsers, &Event {
//!     name: "device.renamed".to_owned(),
//!     data: json!({ "deviceId": device_id }),
//! })
//! .await?;
//! txn.commit().await?;
//! ```
//!
//! An event says what changed, such as a device ID, and never the new state, a secret or
//! personal data: clients fetch the state through the API, which checks that they may see it
//! (AUTH-17). An event must fit in a notification, under 8000 bytes.
//!
//! Browsers connect only with an active sign-in ([`crate::accounts::Authenticator`]), and receive
//! the events of their user. When their sign-in ends, because it is revoked ([`end_sign_in`]) or
//! expires, the server closes the connection with
//! [`NOT_SIGNED_IN`](twoviewer_protocol::close::NOT_SIGNED_IN), and the browser goes to the sign-in
//! page rather than reconnecting. A browser without an active sign-in is closed the same way as
//! soon as it connects: browsers can read the code of a close, but not the status of a refused
//! handshake.
//!
//! Events are not stored. A client that was disconnected, or whose instance lost its database
//! connection for a moment, may have missed some, so it fetches its state after every hello and
//! every resync.
//!
//! On shutdown, every connection is closed with
//! [`SERVICE_RESTART`](twoviewer_protocol::close::SERVICE_RESTART), and clients reconnect to
//! another instance, or to this one once it is back.

mod bus;
mod hub;
mod socket;

use std::sync::Arc;

use axum::{
    Router,
    extract::{State, WebSocketUpgrade},
    http::{HeaderMap, StatusCode, header},
    response::{IntoResponse, Response},
    routing::get,
};
use sea_orm::{ConnectionTrait, DbBackend, DbErr, Statement, prelude::Uuid};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::task::JoinHandle;
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tracing::{Instrument, error, info_span};
use twoviewer_protocol::{AGENT_PATH, BROWSER_PATH, Event};

use crate::{
    accounts::{Authenticator, SignedIn},
    config,
    http::{self, CorrelationId},
    metrics::Metrics,
};

use self::{hub::Hub, socket::Connection};

/// Who receives an event. A connection receives the events of the topics its credential allows:
/// a browser those of its sign-in's user. Host Agent connections are anonymous until Host Agent
/// credentials exist (#33, #36); then topics such as a device are added.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Topic {
    /// Every browser connection.
    Browsers,
    /// Every Host Agent connection.
    Agents,
    /// The browser connections of one user.
    User(Uuid),
    /// The browser connection of one sign-in. Only [`end_sign_in`] publishes to it.
    SignIn(Uuid),
}

/// The event that ends the connections of a sign-in. Connections don't pass it on.
const SIGN_IN_ENDED: &str = "sign_in.ended";

/// Who is at the other end of a connection.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
enum Peer {
    /// A browser, and its sign-in if it has an active one.
    Browser(Option<SignedIn>),
    Agent,
}

impl Peer {
    fn topics(self) -> Vec<Topic> {
        match self {
            Self::Browser(Some(signed_in)) => vec![
                Topic::Browsers,
                Topic::User(signed_in.user_id),
                Topic::SignIn(signed_in.session_id),
            ],
            Self::Browser(None) => Vec::new(),
            Self::Agent => vec![Topic::Agents],
        }
    }

    fn name(self) -> &'static str {
        match self {
            Self::Browser(_) => "browser",
            Self::Agent => "agent",
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum PublishError {
    #[error(
        "the event takes {bytes} bytes, but a notification holds fewer than {MAX_NOTIFICATION_BYTES}"
    )]
    TooLarge { bytes: usize },
    #[error("database error: {0}")]
    Database(#[from] DbErr),
}

/// PostgreSQL's limit on a notification payload, in its default configuration. A payload must be
/// shorter.
const MAX_NOTIFICATION_BYTES: usize = 8000;

/// Sends `event` to the connections of `topic`, on every instance, once the transaction of `db`
/// is committed. Without a transaction, it is sent right away.
pub async fn publish<C: ConnectionTrait>(
    db: &C,
    topic: Topic,
    event: &Event,
) -> Result<(), PublishError> {
    let payload = bus::encode(topic, event);
    if payload.len() >= MAX_NOTIFICATION_BYTES {
        return Err(PublishError::TooLarge {
            bytes: payload.len(),
        });
    }
    db.execute_raw(Statement::from_sql_and_values(
        DbBackend::Postgres,
        "SELECT pg_notify($1, $2)",
        [bus::CHANNEL.into(), payload.into()],
    ))
    .await?;
    Ok(())
}

/// Closes the browser connection of the sign-in `session_id`, on every instance, once the
/// transaction of `db` is committed. [`crate::accounts::sign_in_session::revoke`] does this.
pub async fn end_sign_in<C: ConnectionTrait>(db: &C, session_id: Uuid) -> Result<(), PublishError> {
    publish(
        db,
        Topic::SignIn(session_id),
        &Event {
            name: SIGN_IN_ENDED.to_owned(),
            data: json!({}),
        },
    )
    .await
}

/// The real-time connections of this instance.
pub struct Realtime {
    hub: Arc<Hub>,
    config: config::Realtime,
    /// Checks the sign-in of browser connections.
    authenticator: Authenticator,
    metrics: Arc<Metrics>,
    /// Cancelled on shutdown, which closes every connection.
    closing: CancellationToken,
    connections: TaskTracker,
    listening: JoinHandle<()>,
}

impl Realtime {
    /// Starts receiving events from the database. Start before the HTTPS listener opens, so that
    /// the first connections miss nothing.
    pub async fn start(
        database_url: &str,
        config: config::Realtime,
        authenticator: Authenticator,
        metrics: Arc<Metrics>,
    ) -> Result<Arc<Self>, sqlx::Error> {
        let hub = Arc::new(Hub::default());
        let listener = bus::listen(database_url).await?;
        let listening = tokio::spawn(bus::run(
            listener,
            hub.clone(),
            config.listen_retry_interval,
        ));
        Ok(Arc::new(Self {
            hub,
            config,
            authenticator,
            metrics,
            closing: CancellationToken::new(),
            connections: TaskTracker::new(),
            listening,
        }))
    }

    /// The WebSocket routes for browsers and Host Agents.
    pub fn routes(self: &Arc<Self>) -> Router {
        Router::new()
            .route(BROWSER_PATH, get(browser))
            .route(AGENT_PATH, get(agent))
            .with_state(self.clone())
    }

    /// Closes every connection, and every connection opened from now on, with
    /// [`SERVICE_RESTART`](twoviewer_protocol::close::SERVICE_RESTART).
    pub fn close_all(&self) {
        self.closing.cancel();
    }

    /// Waits until every connection has closed. Call [`Self::close_all`] first.
    pub async fn wait_closed(&self) {
        self.connections.close();
        self.connections.wait().await;
    }

    fn accept(&self, upgrade: WebSocketUpgrade, peer: Peer, correlation_id: Uuid) -> Response {
        let connection = Connection {
            peer,
            hub: self.hub.clone(),
            config: self.config.clone(),
            authenticator: self.authenticator.clone(),
            metrics: self.metrics.clone(),
            closing: self.closing.clone(),
        };
        // Taken now rather than once upgraded, so that shutdown also waits for connections that
        // are being upgraded.
        let tracked = self.connections.token();
        let span = info_span!("realtime", correlation_id = %correlation_id, peer = peer.name());
        upgrade
            .max_message_size(self.config.max_message_bytes)
            .max_frame_size(self.config.max_message_bytes)
            .on_upgrade(move |socket| {
                async move {
                    let _tracked = tracked;
                    connection.serve(socket).await;
                }
                .instrument(span)
            })
    }
}

impl Drop for Realtime {
    fn drop(&mut self) {
        self.listening.abort();
    }
}

async fn browser(
    State(realtime): State<Arc<Realtime>>,
    CorrelationId(correlation_id): CorrelationId,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    // Browsers send the page's origin with every WebSocket handshake, and cookies with it, but no
    // CORS check applies. Without this, any website a user visits could open a connection in
    // their name (SEC-09).
    if !http::same_origin(&headers) {
        return StatusCode::FORBIDDEN.into_response();
    }
    // Without a sign-in, the connection is closed once open, with a code the browser can read.
    match realtime.authenticator.signed_in(&headers).await {
        Ok(signed_in) => realtime.accept(upgrade, Peer::Browser(signed_in), correlation_id),
        Err(err) => {
            error!(error = %err, "cannot check the sign-in of a real-time connection");
            StatusCode::INTERNAL_SERVER_ERROR.into_response()
        }
    }
}

async fn agent(
    State(realtime): State<Arc<Realtime>>,
    CorrelationId(correlation_id): CorrelationId,
    headers: HeaderMap,
    upgrade: WebSocketUpgrade,
) -> Response {
    // Host Agents send no origin, while browsers always do: no web page can pose as an agent.
    if headers.contains_key(header::ORIGIN) {
        return StatusCode::FORBIDDEN.into_response();
    }
    realtime.accept(upgrade, Peer::Agent, correlation_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn events_that_do_not_fit_in_a_notification_are_rejected() {
        let event = Event {
            name: "test.large".to_owned(),
            data: "x".repeat(MAX_NOTIFICATION_BYTES).into(),
        };
        // No database is needed: the size is checked first.
        let db = sea_orm::DatabaseConnection::default();

        let result = publish(&db, Topic::Browsers, &event).await;

        assert!(
            matches!(result, Err(PublishError::TooLarge { bytes }) if bytes > MAX_NOTIFICATION_BYTES),
            "{result:?}"
        );
    }
}
