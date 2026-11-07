//! The connections of this instance, by topic.

use std::{
    collections::{HashMap, HashSet},
    sync::{Arc, Mutex},
};

use tokio::sync::mpsc::{self, error::TrySendError};
use tokio_util::sync::CancellationToken;
use twoviewer_protocol::ServerMessage;

use super::Topic;

#[derive(Default)]
pub(super) struct Hub {
    inner: Mutex<Inner>,
}

#[derive(Default)]
struct Inner {
    next_id: u64,
    connections: HashMap<u64, Subscriber>,
    topics: HashMap<Topic, HashSet<u64>>,
}

struct Subscriber {
    /// The connection's messages waiting to be sent.
    queue: mpsc::Sender<ServerMessage>,
    /// Cancelled when the queue is full, which closes the connection.
    overflowed: CancellationToken,
}

impl Hub {
    /// Delivers the messages of `topics` to `queue` until the subscription is dropped. If the
    /// queue is full when a message arrives, `overflowed` is cancelled and the message dropped.
    pub(super) fn subscribe(
        self: &Arc<Self>,
        topics: &[Topic],
        queue: mpsc::Sender<ServerMessage>,
        overflowed: CancellationToken,
    ) -> Subscription {
        let mut inner = self.inner.lock().expect("the hub lock is never poisoned");
        let id = inner.next_id;
        inner.next_id += 1;
        inner
            .connections
            .insert(id, Subscriber { queue, overflowed });
        for topic in topics {
            inner.topics.entry(*topic).or_default().insert(id);
        }
        Subscription {
            hub: self.clone(),
            id,
            topics: topics.to_vec(),
        }
    }

    /// Delivers `message` to every connection subscribed to `topic`.
    pub(super) fn dispatch(&self, topic: Topic, message: &ServerMessage) {
        let inner = self.inner.lock().expect("the hub lock is never poisoned");
        for id in inner.topics.get(&topic).into_iter().flatten() {
            if let Some(subscriber) = inner.connections.get(id) {
                subscriber.deliver(message.clone());
            }
        }
    }

    /// Delivers `message` to every connection.
    pub(super) fn broadcast(&self, message: &ServerMessage) {
        let inner = self.inner.lock().expect("the hub lock is never poisoned");
        for subscriber in inner.connections.values() {
            subscriber.deliver(message.clone());
        }
    }
}

impl Subscriber {
    fn deliver(&self, message: ServerMessage) {
        match self.queue.try_send(message) {
            // Closed: the connection is closing, and its subscription is about to be dropped.
            Ok(()) | Err(TrySendError::Closed(_)) => {}
            // Dropping one message would leave the client unaware; it resyncs when it reconnects.
            Err(TrySendError::Full(_)) => self.overflowed.cancel(),
        }
    }
}

/// A connection's subscription, which ends when dropped.
pub(super) struct Subscription {
    hub: Arc<Hub>,
    id: u64,
    topics: Vec<Topic>,
}

impl Drop for Subscription {
    fn drop(&mut self) {
        let mut inner = self
            .hub
            .inner
            .lock()
            .expect("the hub lock is never poisoned");
        inner.connections.remove(&self.id);
        for topic in &self.topics {
            if let Some(ids) = inner.topics.get_mut(topic) {
                ids.remove(&self.id);
                if ids.is_empty() {
                    inner.topics.remove(topic);
                }
            }
        }
    }
}

#[cfg(test)]
mod tests {
    use twoviewer_protocol::Event;

    use super::*;

    fn event(name: &str) -> ServerMessage {
        ServerMessage::Event(Event {
            name: name.to_owned(),
            data: serde_json::Value::Null,
        })
    }

    /// A subscription to `topics` with a queue of `capacity`.
    fn subscribe(
        hub: &Arc<Hub>,
        topics: &[Topic],
        capacity: usize,
    ) -> (
        Subscription,
        mpsc::Receiver<ServerMessage>,
        CancellationToken,
    ) {
        let (queue, received) = mpsc::channel(capacity);
        let overflowed = CancellationToken::new();
        let subscription = hub.subscribe(topics, queue, overflowed.clone());
        (subscription, received, overflowed)
    }

    #[test]
    fn events_reach_only_the_subscribers_of_their_topic() {
        let hub = Arc::new(Hub::default());
        let (_browser, mut browser, _) = subscribe(&hub, &[Topic::Browsers], 8);
        let (_agent, mut agent, _) = subscribe(&hub, &[Topic::Agents], 8);

        hub.dispatch(Topic::Agents, &event("for.agents"));

        assert_eq!(agent.try_recv().unwrap(), event("for.agents"));
        assert!(browser.try_recv().is_err());
    }

    #[test]
    fn broadcasts_reach_every_connection_once() {
        let hub = Arc::new(Hub::default());
        let (_both, mut both, _) = subscribe(&hub, &[Topic::Browsers, Topic::Agents], 8);
        let (_agent, mut agent, _) = subscribe(&hub, &[Topic::Agents], 8);

        hub.broadcast(&ServerMessage::Resync);

        assert_eq!(both.try_recv().unwrap(), ServerMessage::Resync);
        assert!(both.try_recv().is_err());
        assert_eq!(agent.try_recv().unwrap(), ServerMessage::Resync);
    }

    #[test]
    fn a_full_queue_marks_its_connection_as_overflowed() {
        let hub = Arc::new(Hub::default());
        let (_slow, mut slow, overflowed) = subscribe(&hub, &[Topic::Browsers], 1);

        hub.dispatch(Topic::Browsers, &event("first"));
        assert!(!overflowed.is_cancelled());
        hub.dispatch(Topic::Browsers, &event("second"));

        assert!(overflowed.is_cancelled());
        assert_eq!(slow.try_recv().unwrap(), event("first"));
    }

    #[test]
    fn dropped_subscriptions_receive_nothing_and_leave_nothing_behind() {
        let hub = Arc::new(Hub::default());
        let (subscription, mut received, _) = subscribe(&hub, &[Topic::Browsers], 8);

        drop(subscription);
        hub.dispatch(Topic::Browsers, &event("late"));

        assert!(received.try_recv().is_err());
        let inner = hub.inner.lock().unwrap();
        assert!(inner.connections.is_empty());
        assert!(inner.topics.is_empty());
    }
}
