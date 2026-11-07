//! The agent's connection against the real server: it connects, receives events, and reconnects
//! once a restarted server is back (REL-05). Needs Docker.

// The server's test helpers: a test database, and a server with a certificate the agent trusts.
#[path = "../../server/tests/common/mod.rs"]
mod common;

use std::time::Duration;

use common::{
    postgres,
    server::{client_tls, free_address, start},
};
use serde_json::json;
use tokio::sync::{mpsc, watch};
use twoviewer_agent::connection::{Connection, Received, State};
use twoviewer_protocol::Event;
use twoviewer_server::{
    db,
    realtime::{self, Topic},
};

const ONLINE: &str = "twoviewer_host_agents_online";

async fn wait_for(state: &mut watch::Receiver<State>, expected: State) {
    tokio::time::timeout(
        Duration::from_secs(30),
        state.wait_for(|state| *state == expected),
    )
    .await
    .unwrap_or_else(|_| panic!("never {expected:?}"))
    .unwrap();
}

async fn next(received: &mut mpsc::UnboundedReceiver<Received>) -> Received {
    tokio::time::timeout(Duration::from_secs(10), received.recv())
        .await
        .expect("a message within 10 seconds")
        .unwrap()
}

#[tokio::test]
async fn the_agent_stays_connected_across_a_server_restart() {
    let (_postgres, url) = postgres().await;
    let address = free_address();
    let server = start(&url, |config| config.http.listen = address).await;
    server.wait_for("/readyz", 200).await;

    let connection = Connection::new(&server.origin(), client_tls()).unwrap();
    let (received_tx, mut received) = mpsc::unbounded_channel();
    let (state_tx, mut state) = watch::channel(State::Connecting);
    let running = tokio::spawn(connection.run(received_tx, state_tx));

    wait_for(&mut state, State::Connected).await;
    assert_eq!(next(&mut received).await, Received::Resync);
    server.wait_for_metric(ONLINE, 1.0).await;

    let event = Event {
        name: "test.for_agents".to_owned(),
        data: json!({"id": 1}),
    };
    let db = db::connect(&url).await.unwrap();
    realtime::publish(&db, Topic::Agents, &event).await.unwrap();
    assert_eq!(next(&mut received).await, Received::Event(event));

    server.shut_down().await.unwrap();
    wait_for(&mut state, State::Disconnected).await;

    let server = start(&url, |config| config.http.listen = address).await;
    wait_for(&mut state, State::Connected).await;
    // It fetches everything again, since it may have missed events.
    assert_eq!(next(&mut received).await, Received::Resync);
    server.wait_for_metric(ONLINE, 1.0).await;

    running.abort();
    server.wait_for_metric(ONLINE, 0.0).await;
    server.shut_down().await.unwrap();
}
