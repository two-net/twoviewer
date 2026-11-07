//! The real-time connection (IF-09) against a real PostgreSQL: hello and heartbeats, which
//! clients may connect, events across instances, resyncs and shutdown. Needs Docker.

mod common;

use std::time::{Duration, Instant};

use common::{
    postgres,
    server::{Server, Socket, connect, start},
};
use futures_util::{SinkExt, StreamExt};
use sea_orm::TransactionTrait;
use serde_json::json;
use tokio_tungstenite::tungstenite::{self, Message, protocol::frame::coding::CloseCode};
use twoviewer_protocol::{AGENT_PATH, BROWSER_PATH, ClientMessage, Event, ServerMessage, close};
use twoviewer_server::{
    db,
    realtime::{self, Topic},
};

const ONLINE: &str = "twoviewer_host_agents_online";

/// Starts a server against `url` and waits until it is ready.
async fn ready(url: &str, configure: impl FnOnce(&mut twoviewer_server::config::Config)) -> Server {
    let server = start(url, configure).await;
    server.wait_for("/readyz", 200).await;
    server
}

async fn browser(server: &Server) -> Socket {
    let mut socket = connect(server, BROWSER_PATH, Some(&server.origin()))
        .await
        .unwrap();
    assert!(matches!(
        next(&mut socket).await,
        ServerMessage::Hello { .. }
    ));
    socket
}

async fn agent(server: &Server) -> Socket {
    let mut socket = connect(server, AGENT_PATH, None).await.unwrap();
    assert!(matches!(
        next(&mut socket).await,
        ServerMessage::Hello { .. }
    ));
    socket
}

/// The next frame, within 5 seconds.
async fn frame(socket: &mut Socket) -> Option<Message> {
    tokio::time::timeout(Duration::from_secs(5), socket.next())
        .await
        .expect("a message within 5 seconds")
        .and_then(Result::ok)
}

/// The next server message, as it comes.
async fn next(socket: &mut Socket) -> ServerMessage {
    match frame(socket).await {
        Some(Message::Text(text)) => serde_json::from_str(&text).unwrap(),
        other => panic!("expected a server message, got {other:?}"),
    }
}

/// The next server message other than a heartbeat, within 10 seconds. It answers heartbeats.
async fn next_but_heartbeats(socket: &mut Socket) -> ServerMessage {
    let next = async {
        loop {
            match next(socket).await {
                ServerMessage::Heartbeat => answer(socket).await,
                message => return message,
            }
        }
    };
    tokio::time::timeout(Duration::from_secs(10), next)
        .await
        .expect("a message other than a heartbeat within 10 seconds")
}

async fn answer(socket: &mut Socket) {
    let heartbeat = serde_json::to_string(&ClientMessage::Heartbeat).unwrap();
    socket.send(Message::text(heartbeat)).await.unwrap();
}

/// The code of the close frame that ends the connection, skipping other messages.
async fn closed_with(socket: &mut Socket) -> Option<u16> {
    loop {
        match frame(socket).await {
            Some(Message::Close(frame)) => return frame.map(|frame| u16::from(frame.code)),
            None => return None,
            Some(_) => {}
        }
    }
}

fn event(name: &str) -> Event {
    Event {
        name: name.to_owned(),
        data: json!({"id": name}),
    }
}

fn status(result: Result<Socket, tungstenite::Error>) -> u16 {
    match result {
        Ok(_) => 101,
        Err(tungstenite::Error::Http(response)) => response.status().as_u16(),
        Err(err) => panic!("{err}"),
    }
}

#[tokio::test]
async fn the_server_says_hello_then_sends_heartbeats() {
    let (_postgres, url) = postgres().await;
    let server = ready(&url, |config| {
        config.realtime.heartbeat_interval = Duration::from_millis(100);
        config.realtime.heartbeat_timeout = Duration::from_millis(1500);
    })
    .await;
    let mut socket = connect(&server, BROWSER_PATH, Some(&server.origin()))
        .await
        .unwrap();

    assert_eq!(
        next(&mut socket).await,
        ServerMessage::Hello {
            heartbeat_timeout_ms: 1500
        }
    );
    assert_eq!(next(&mut socket).await, ServerMessage::Heartbeat);

    // Answering keeps the connection open well beyond the timeout.
    let until = Instant::now() + Duration::from_secs(3);
    while Instant::now() < until {
        assert_eq!(next(&mut socket).await, ServerMessage::Heartbeat);
        answer(&mut socket).await;
    }
    server.shut_down().await.unwrap();
}

#[tokio::test]
async fn silent_clients_are_disconnected() {
    let (_postgres, url) = postgres().await;
    let server = ready(&url, |config| {
        config.realtime.heartbeat_interval = Duration::from_millis(100);
        config.realtime.heartbeat_timeout = Duration::from_millis(500);
    })
    .await;
    let mut socket = agent(&server).await;
    server.wait_for_metric(ONLINE, 1.0).await;

    let started = Instant::now();
    assert_eq!(closed_with(&mut socket).await, Some(close::GOING_AWAY));

    assert!(started.elapsed() < Duration::from_secs(2));
    // The agent is offline as soon as its connection is gone.
    server.wait_for_metric(ONLINE, 0.0).await;
    server.shut_down().await.unwrap();
}

#[tokio::test]
async fn only_pages_of_this_server_and_host_agents_may_connect() {
    let (_postgres, url) = postgres().await;
    let server = ready(&url, |_| {}).await;
    let origin = server.origin();

    for (path, origin, expected) in [
        (BROWSER_PATH, Some(origin.as_str()), 101),
        (BROWSER_PATH, Some("https://evil.example"), 403),
        (BROWSER_PATH, Some("http://localhost"), 403),
        (BROWSER_PATH, None, 403),
        (AGENT_PATH, None, 101),
        // A web page cannot pose as a Host Agent.
        (AGENT_PATH, Some(origin.as_str()), 403),
        (AGENT_PATH, Some("https://evil.example"), 403),
    ] {
        assert_eq!(
            status(connect(&server, path, origin).await),
            expected,
            "{path} from {origin:?}"
        );
    }
    server.shut_down().await.unwrap();
}

#[tokio::test]
async fn host_agents_count_as_online_while_connected() {
    let (_postgres, url) = postgres().await;
    let server = ready(&url, |_| {}).await;

    let _browser = browser(&server).await;
    let first = agent(&server).await;
    let mut second = agent(&server).await;
    server.wait_for_metric(ONLINE, 2.0).await;

    drop(first);
    second.close(None).await.unwrap();

    server.wait_for_metric(ONLINE, 0.0).await;
    server.shut_down().await.unwrap();
}

#[tokio::test]
async fn invalid_messages_close_the_connection() {
    let (_postgres, url) = postgres().await;
    let server = ready(&url, |config| {
        config.realtime.max_message_bytes = 1024;
    })
    .await;

    let mut socket = browser(&server).await;
    socket.send(Message::text("nonsense")).await.unwrap();
    assert_eq!(
        closed_with(&mut socket).await,
        Some(close::POLICY_VIOLATION)
    );

    let mut socket = browser(&server).await;
    socket.send(Message::binary(vec![1, 2, 3])).await.unwrap();
    assert_eq!(
        closed_with(&mut socket).await,
        Some(close::UNSUPPORTED_DATA)
    );

    let mut socket = browser(&server).await;
    socket.send(Message::text("x".repeat(2048))).await.unwrap();
    assert_ne!(
        closed_with(&mut socket).await,
        Some(u16::from(CloseCode::Normal))
    );
    server.shut_down().await.unwrap();
}

#[tokio::test]
async fn events_reach_the_connections_of_their_topic_on_every_instance() {
    let (_postgres, url) = postgres().await;
    let first = ready(&url, |_| {}).await;
    let second = ready(&url, |_| {}).await;
    let mut on_first = browser(&first).await;
    let mut on_second = browser(&second).await;
    let mut agent = agent(&first).await;
    let db = db::connect(&url).await.unwrap();

    for (topic, name) in [
        (Topic::Browsers, "for.browsers"),
        (Topic::Agents, "for.agents"),
        (Topic::Browsers, "for.browsers.again"),
    ] {
        realtime::publish(&db, topic, &event(name)).await.unwrap();
    }

    for socket in [&mut on_first, &mut on_second] {
        assert_eq!(
            next_but_heartbeats(socket).await,
            ServerMessage::Event(event("for.browsers"))
        );
        assert_eq!(
            next_but_heartbeats(socket).await,
            ServerMessage::Event(event("for.browsers.again"))
        );
    }
    assert_eq!(
        next_but_heartbeats(&mut agent).await,
        ServerMessage::Event(event("for.agents"))
    );
    first.shut_down().await.unwrap();
    second.shut_down().await.unwrap();
}

#[tokio::test]
async fn events_are_sent_only_once_their_transaction_commits() {
    let (_postgres, url) = postgres().await;
    let server = ready(&url, |_| {}).await;
    let mut socket = browser(&server).await;
    let db = db::connect(&url).await.unwrap();

    let txn = db.begin().await.unwrap();
    realtime::publish(&txn, Topic::Browsers, &event("rolled.back"))
        .await
        .unwrap();
    txn.rollback().await.unwrap();
    let txn = db.begin().await.unwrap();
    realtime::publish(&txn, Topic::Browsers, &event("committed"))
        .await
        .unwrap();
    txn.commit().await.unwrap();

    assert_eq!(
        next_but_heartbeats(&mut socket).await,
        ServerMessage::Event(event("committed"))
    );
    server.shut_down().await.unwrap();
}

#[tokio::test]
async fn clients_resync_when_their_instance_may_have_missed_events() {
    let (_postgres, url) = postgres().await;
    let server = ready(&url, |_| {}).await;
    let mut socket = browser(&server).await;
    let db = db::connect(&url).await.unwrap();

    // Ends the connection on which the server listens for events.
    let terminated: Vec<bool> = sqlx::query_scalar(
        "SELECT pg_terminate_backend(pid) FROM pg_stat_activity WHERE query LIKE 'LISTEN %'",
    )
    .fetch_all(db.get_postgres_connection_pool())
    .await
    .unwrap();
    assert_eq!(terminated, [true]);

    assert_eq!(
        next_but_heartbeats(&mut socket).await,
        ServerMessage::Resync
    );
    // And the server listens again.
    realtime::publish(&db, Topic::Browsers, &event("after.resync"))
        .await
        .unwrap();
    assert_eq!(
        next_but_heartbeats(&mut socket).await,
        ServerMessage::Event(event("after.resync"))
    );
    server.shut_down().await.unwrap();
}

#[tokio::test]
async fn shutdown_closes_connections_for_a_restart_without_waiting_for_them() {
    let (_postgres, url) = postgres().await;
    let server = ready(&url, |config| {
        config.http.shutdown_timeout = Duration::from_secs(30);
    })
    .await;
    let mut browser = browser(&server).await;
    let mut agent = agent(&server).await;

    let started = Instant::now();
    let stopped = server.stop();

    assert_eq!(
        closed_with(&mut browser).await,
        Some(close::SERVICE_RESTART)
    );
    assert_eq!(closed_with(&mut agent).await, Some(close::SERVICE_RESTART));
    tokio::time::timeout(Duration::from_secs(5), stopped)
        .await
        .expect("the server stops without waiting for the shutdown timeout")
        .unwrap()
        .unwrap();
    assert!(started.elapsed() < Duration::from_secs(5));
}
