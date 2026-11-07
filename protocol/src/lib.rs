//! The real-time connection between the server and its clients, browsers and Host Agents (IF-09).
//!
//! A client opens a WebSocket on the HTTPS listener: browsers on [`BROWSER_PATH`], Host Agents on
//! [`AGENT_PATH`]. Every message is a JSON text message with a `type`:
//!
//! 1. The server sends [`ServerMessage::Hello`] first.
//! 2. The server sends [`ServerMessage::Heartbeat`] at a fixed interval, and the client answers
//!    each one with [`ClientMessage::Heartbeat`]. Either side treats a connection that has been
//!    silent for the `heartbeat_timeout_ms` of the hello as lost: the server closes it, the
//!    client reconnects.
//! 3. [`ServerMessage::Event`] says that something changed, not what it changed to: the client
//!    fetches the current state through the API.
//! 4. After every hello and every [`ServerMessage::Resync`], events may have been missed, so the
//!    client fetches all of the state it shows.
//!
//! Clients reconnect after every close, with randomized, increasing delays (REL-05). They ignore
//! message types they do not know, so that the server can add some before every Host Agent is
//! updated. The server closes the connection when a client sends anything else than what is
//! defined here.
//!
//! The web client has a copy of these types in `web/src/app/shared/connection/messages.ts`.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// Where browsers open the connection.
pub const BROWSER_PATH: &str = "/api/realtime";
/// Where Host Agents open the connection.
pub const AGENT_PATH: &str = "/api/agent/realtime";

/// A message from the server.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(
    tag = "type",
    rename_all = "camelCase",
    rename_all_fields = "camelCase"
)]
pub enum ServerMessage {
    /// The first message on every connection.
    Hello {
        /// How long the client waits for a message from the server before it reconnects.
        heartbeat_timeout_ms: u64,
    },
    /// Answer it with [`ClientMessage::Heartbeat`].
    Heartbeat,
    /// Events may have been missed: fetch the current state.
    Resync,
    Event(Event),
    /// A message type that this version does not know. Never sent.
    #[serde(other, skip_serializing)]
    Unknown,
}

/// A change that the client may show. Its `data` identifies what changed, such as a device ID,
/// but does not carry the new state, which the client fetches through the API.
#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Event {
    /// What happened, as `<feature>.<change>`, such as `device.status`.
    pub name: String,
    #[serde(default)]
    pub data: Value,
}

/// A message from a client.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum ClientMessage {
    /// The answer to [`ServerMessage::Heartbeat`].
    Heartbeat,
}

/// The close codes the server uses (RFC 6455, section 7.4.1). Clients reconnect after any of
/// them.
pub mod close {
    /// The connection was silent for longer than the heartbeat timeout.
    pub const GOING_AWAY: u16 = 1001;
    /// The client sent a binary message.
    pub const UNSUPPORTED_DATA: u16 = 1003;
    /// The client sent a message that is not a [`ClientMessage`](super::ClientMessage).
    pub const POLICY_VIOLATION: u16 = 1008;
    /// The server is shutting down. Another instance, or this one once restarted, takes over.
    pub const SERVICE_RESTART: u16 = 1012;
    /// The client did not keep up with its messages.
    pub const TRY_AGAIN_LATER: u16 = 1013;
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn server_messages_are_tagged_with_their_type() {
        for (message, encoded) in [
            (
                ServerMessage::Hello {
                    heartbeat_timeout_ms: 8000,
                },
                json!({"type": "hello", "heartbeatTimeoutMs": 8000}),
            ),
            (ServerMessage::Heartbeat, json!({"type": "heartbeat"})),
            (ServerMessage::Resync, json!({"type": "resync"})),
            (
                ServerMessage::Event(Event {
                    name: "device.status".to_owned(),
                    data: json!({"deviceId": "1"}),
                }),
                json!({"type": "event", "name": "device.status", "data": {"deviceId": "1"}}),
            ),
        ] {
            assert_eq!(serde_json::to_value(&message).unwrap(), encoded);
            assert_eq!(
                serde_json::from_value::<ServerMessage>(encoded).unwrap(),
                message
            );
        }
    }

    #[test]
    fn clients_read_unknown_server_messages_as_unknown() {
        let message: ServerMessage =
            serde_json::from_value(json!({"type": "somethingNew", "field": 1})).unwrap();

        assert_eq!(message, ServerMessage::Unknown);
    }

    #[test]
    fn the_server_reads_only_known_client_messages() {
        assert_eq!(
            serde_json::from_str::<ClientMessage>(r#"{"type":"heartbeat"}"#).unwrap(),
            ClientMessage::Heartbeat
        );
        for invalid in [r#"{"type":"somethingNew"}"#, r#"{}"#, "heartbeat"] {
            assert!(
                serde_json::from_str::<ClientMessage>(invalid).is_err(),
                "{invalid}"
            );
        }
    }
}
