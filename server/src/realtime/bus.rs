//! Events between instances, through PostgreSQL notifications on one channel.
//!
//! Every instance listens on its own database connection, outside the pool. When that connection
//! is lost, notifications sent in the meantime are lost too, so once it is back, every connection
//! of the instance is told to resync.

use std::{sync::Arc, time::Duration};

use serde::{Deserialize, Serialize};
use sqlx::postgres::PgListener;
use tracing::{info, warn};
use twoviewer_protocol::{Event, ServerMessage};

use super::{Topic, hub::Hub};

/// The notification channel of real-time events.
pub(super) const CHANNEL: &str = "twoviewer_realtime";

/// What a notification carries.
#[derive(Serialize)]
struct Outgoing<'a> {
    topic: Topic,
    event: &'a Event,
}

#[derive(Deserialize)]
struct Incoming {
    topic: Topic,
    event: Event,
}

/// The payload of the notification for `event`.
pub(super) fn encode(topic: Topic, event: &Event) -> String {
    serde_json::to_string(&Outgoing { topic, event }).expect("an event is valid JSON")
}

/// Opens the connection that receives the events of every instance.
pub(super) async fn listen(database_url: &str) -> Result<PgListener, sqlx::Error> {
    let mut listener = PgListener::connect(database_url).await?;
    listener.listen(CHANNEL).await?;
    Ok(listener)
}

/// Delivers the events that `listener` receives to the connections of `hub`. Runs until it is
/// aborted.
pub(super) async fn run(mut listener: PgListener, hub: Arc<Hub>, retry_interval: Duration) {
    loop {
        match listener.try_recv().await {
            Ok(Some(notification)) => {
                match serde_json::from_str::<Incoming>(notification.payload()) {
                    Ok(Incoming { topic, event }) => {
                        hub.dispatch(topic, &ServerMessage::Event(event));
                    }
                    Err(err) => warn!(error = %err, "ignoring an invalid real-time event"),
                }
            }
            // The connection was lost, and is already being reestablished.
            Ok(None) => {
                warn!("lost the database connection that receives real-time events");
                reconnect(&mut listener, retry_interval).await;
                hub.broadcast(&ServerMessage::Resync);
            }
            Err(err) => {
                warn!(error = %err, "cannot receive real-time events");
                reconnect(&mut listener, retry_interval).await;
                hub.broadcast(&ServerMessage::Resync);
            }
        }
    }
}

/// Returns once `listener` is connected and listening again.
async fn reconnect(listener: &mut PgListener, retry_interval: Duration) {
    loop {
        // A query on the listener reconnects it if needed, and listens again on its channels.
        match sqlx::query("SELECT 1").execute(&mut *listener).await {
            Ok(_) => {
                info!("receiving real-time events again");
                return;
            }
            Err(err) => {
                warn!(error = %err, "cannot reconnect to receive real-time events; retrying");
                tokio::time::sleep(retry_interval).await;
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use serde_json::json;

    use super::*;

    #[test]
    fn notifications_carry_the_topic_and_the_event() {
        let event = Event {
            name: "device.status".to_owned(),
            data: json!({"deviceId": "1"}),
        };

        let payload = encode(Topic::Agents, &event);

        let incoming: Incoming = serde_json::from_str(&payload).unwrap();
        assert_eq!(incoming.topic, Topic::Agents);
        assert_eq!(incoming.event, event);
        assert_eq!(
            serde_json::from_str::<serde_json::Value>(&payload).unwrap()["topic"],
            "agents"
        );
    }
}
