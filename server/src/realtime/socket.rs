//! One connection: hello, heartbeats, events, and closing it.

use std::{pin::pin, sync::Arc};

use axum::extract::ws::{CloseFrame, Message, Utf8Bytes, WebSocket};
use futures_util::{SinkExt, StreamExt, stream::SplitSink};
use prometheus_client::metrics::gauge::Gauge;
use tokio::{
    sync::mpsc,
    time::{Instant, MissedTickBehavior},
};
use tokio_util::sync::CancellationToken;
use tracing::debug;
use twoviewer_protocol::{ClientMessage, ServerMessage, close};

use super::{Peer, hub::Hub};
use crate::{config, metrics::Metrics};

pub(super) struct Connection {
    pub(super) peer: Peer,
    pub(super) hub: Arc<Hub>,
    pub(super) config: config::Realtime,
    pub(super) metrics: Arc<Metrics>,
    pub(super) closing: CancellationToken,
}

impl Connection {
    /// Serves the connection until it closes.
    pub(super) async fn serve(self, socket: WebSocket) {
        let (queue, mut queued) = mpsc::channel(self.config.send_queue_length);
        let overflowed = CancellationToken::new();
        let _subscription = self
            .hub
            .subscribe(self.peer.topics(), queue, overflowed.clone());
        let _online =
            (self.peer == Peer::Agent).then(|| Online::new(&self.metrics.host_agents_online));
        debug!("real-time connection opened");

        let reason = self.run(socket, &mut queued, &overflowed).await;

        debug!(reason, "real-time connection closed");
    }

    /// Exchanges messages until the connection closes, and returns why it closed.
    async fn run(
        &self,
        socket: WebSocket,
        queued: &mut mpsc::Receiver<ServerMessage>,
        overflowed: &CancellationToken,
    ) -> &'static str {
        let interval = self.config.heartbeat_interval;
        let timeout = self.config.heartbeat_timeout;
        let (mut sink, mut stream) = socket.split();

        let hello = ServerMessage::Hello {
            heartbeat_timeout_ms: u64::try_from(timeout.as_millis()).unwrap_or(u64::MAX),
        };
        if !self.send(&mut sink, &hello).await {
            return "cannot send";
        }
        let mut heartbeats = tokio::time::interval_at(Instant::now() + interval, interval);
        heartbeats.set_missed_tick_behavior(MissedTickBehavior::Delay);
        let mut silence = pin!(tokio::time::sleep(timeout));

        loop {
            let message = tokio::select! {
                biased;
                () = self.closing.cancelled() => {
                    return self.close(&mut sink, close::SERVICE_RESTART, "server restarting").await;
                }
                () = overflowed.cancelled() => {
                    return self.close(&mut sink, close::TRY_AGAIN_LATER, "too many messages waiting").await;
                }
                () = &mut silence => {
                    return self.close(&mut sink, close::GOING_AWAY, "no heartbeat").await;
                }
                received = stream.next() => {
                    // Anything from the client shows that it is still there.
                    silence.as_mut().reset(Instant::now() + timeout);
                    match received {
                        Some(Ok(Message::Text(text))) => match serde_json::from_str(&text) {
                            Ok(ClientMessage::Heartbeat) => continue,
                            Err(_) => {
                                return self.close(&mut sink, close::POLICY_VIOLATION, "invalid message").await;
                            }
                        },
                        Some(Ok(Message::Binary(_))) => {
                            return self.close(&mut sink, close::UNSUPPORTED_DATA, "binary message").await;
                        }
                        // The WebSocket library answers pings itself.
                        Some(Ok(Message::Ping(_) | Message::Pong(_))) => continue,
                        Some(Ok(Message::Close(_))) | None => return "closed by the client",
                        // Including messages above `max_message_bytes`.
                        Some(Err(_)) => return "connection error",
                    }
                }
                Some(message) = queued.recv() => message,
                _ = heartbeats.tick() => ServerMessage::Heartbeat,
            };
            if !self.send(&mut sink, &message).await {
                return "cannot send";
            }
        }
    }

    /// Sends `message`, and returns whether it was sent within the heartbeat timeout. A client
    /// that does not read its messages would otherwise hold the connection open.
    async fn send(
        &self,
        sink: &mut SplitSink<WebSocket, Message>,
        message: &ServerMessage,
    ) -> bool {
        let text = serde_json::to_string(message).expect("a server message is valid JSON");
        let sending = sink.send(Message::Text(text.into()));
        matches!(
            tokio::time::timeout(self.config.heartbeat_timeout, sending).await,
            Ok(Ok(()))
        )
    }

    /// Tells the client why the connection closes, and returns the reason.
    async fn close(
        &self,
        sink: &mut SplitSink<WebSocket, Message>,
        code: u16,
        reason: &'static str,
    ) -> &'static str {
        let frame = CloseFrame {
            code,
            reason: Utf8Bytes::from_static(reason),
        };
        let closing = sink.send(Message::Close(Some(frame)));
        let _ = tokio::time::timeout(self.config.heartbeat_timeout, closing).await;
        reason
    }
}

/// Counts a Host Agent as online while it exists.
struct Online(Gauge);

impl Online {
    fn new(gauge: &Gauge) -> Self {
        gauge.inc();
        Self(gauge.clone())
    }
}

impl Drop for Online {
    fn drop(&mut self) {
        self.0.dec();
    }
}
