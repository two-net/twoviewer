//! The real-time connection (IF-09) against a real PostgreSQL: hello and heartbeats, which
//! clients may connect, events across instances, resyncs, the end of a browser's sign-in, and
//! shutdown. Needs Docker.

mod common;

use std::time::{Duration, Instant};

use chrono::{TimeDelta, Utc};
use common::{
    postgres,
    server::{Server, Socket, connect, start},
};
use futures_util::{SinkExt, StreamExt};
use sea_orm::{
    ActiveModelTrait, ConnectionTrait, DatabaseConnection, DbBackend, EntityTrait, IntoActiveModel,
    Set, Statement, TransactionTrait, prelude::Uuid,
};
use serde_json::json;
use tokio_tungstenite::tungstenite::{self, Message, protocol::frame::coding::CloseCode};
use twoviewer_protocol::{AGENT_PATH, BROWSER_PATH, ClientMessage, Event, ServerMessage, close};
use twoviewer_server::{
    accounts::{
        sign_in_session::{self, Revoke},
        token::Token,
    },
    audit::{Action, Actor},
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

/// A browser's sign-in.
struct SignIn {
    session_id: Uuid,
    cookie: String,
}

/// A new account, with an address of its own.
async fn create_user(db: &DatabaseConnection) -> Uuid {
    let row = db
        .query_one_raw(Statement::from_sql_and_values(
            DbBackend::Postgres,
            "INSERT INTO user_account (email, password_hash, display_name, accepted_terms_version, \
             accepted_privacy_version, terms_accepted_at) \
             VALUES ($1, 'not a hash', 'Ann Lee', '1', '1', now()) RETURNING id",
            [format!("{}@example.com", Uuid::new_v4()).into()],
        ))
        .await
        .unwrap()
        .unwrap();
    row.try_get("", "id").unwrap()
}

/// A new sign-in of `user_id` that ends after `lasts` unless it is used.
async fn sign_in(db: &DatabaseConnection, user_id: Uuid, lasts: TimeDelta) -> SignIn {
    let token = Token::generate();
    let now = Utc::now();
    let session = sign_in_session::ActiveModel {
        user_id: Set(user_id),
        token_hash: Set(token.hash()),
        created_at: Set(now),
        last_active_at: Set(now),
        expires_at: Set(now + lasts),
        ip_address: Set("203.0.113.7/32".parse().unwrap()),
        client_description: Set("Firefox on Linux".to_owned()),
        revoked_at: Set(None),
        ..Default::default()
    }
    .insert(db)
    .await
    .unwrap();
    SignIn {
        session_id: session.id,
        cookie: format!("__Host-twoviewer-session={}", token.expose()),
    }
}

/// A browser connection of a new user, after its hello.
async fn browser(server: &Server, url: &str) -> Socket {
    let db = db::connect(url).await.unwrap();
    let sign_in = sign_in(&db, create_user(&db).await, TimeDelta::hours(1)).await;
    browser_of(server, &sign_in).await
}

/// The browser connection of `sign_in`, after its hello.
async fn browser_of(server: &Server, sign_in: &SignIn) -> Socket {
    let mut socket = connect(
        server,
        BROWSER_PATH,
        Some(&server.origin()),
        Some(&sign_in.cookie),
    )
    .await
    .unwrap();
    assert!(matches!(
        next(&mut socket).await,
        ServerMessage::Hello { .. }
    ));
    socket
}

async fn agent(server: &Server) -> Socket {
    let mut socket = connect(server, AGENT_PATH, None, None).await.unwrap();
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
    let db = db::connect(&url).await.unwrap();
    let sign_in = sign_in(&db, create_user(&db).await, TimeDelta::hours(1)).await;
    let mut socket = connect(
        &server,
        BROWSER_PATH,
        Some(&server.origin()),
        Some(&sign_in.cookie),
    )
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

    // Browsers without a sign-in are let in, then closed with a code they can read.
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
            status(connect(&server, path, origin, None).await),
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

    let _browser = browser(&server, &url).await;
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

    let mut socket = browser(&server, &url).await;
    socket.send(Message::text("nonsense")).await.unwrap();
    assert_eq!(
        closed_with(&mut socket).await,
        Some(close::POLICY_VIOLATION)
    );

    let mut socket = browser(&server, &url).await;
    socket.send(Message::binary(vec![1, 2, 3])).await.unwrap();
    assert_eq!(
        closed_with(&mut socket).await,
        Some(close::UNSUPPORTED_DATA)
    );

    let mut socket = browser(&server, &url).await;
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
    let mut on_first = browser(&first, &url).await;
    let mut on_second = browser(&second, &url).await;
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
    let mut socket = browser(&server, &url).await;
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
    let mut socket = browser(&server, &url).await;
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
async fn browsers_without_an_active_sign_in_are_closed_at_once() {
    let (_postgres, url) = postgres().await;
    let server = ready(&url, |_| {}).await;
    let db = db::connect(&url).await.unwrap();
    let ann = create_user(&db).await;
    let expired = sign_in(&db, ann, TimeDelta::seconds(-1)).await;
    let revoked = sign_in(&db, ann, TimeDelta::hours(1)).await;
    sign_in_session::revoke(
        &db,
        ann,
        Revoke::One(revoked.session_id),
        Action::SignOut,
        Actor::System,
        Uuid::new_v4(),
    )
    .await
    .unwrap();

    for cookie in [
        None,
        Some("__Host-twoviewer-session=nonsense"),
        Some(expired.cookie.as_str()),
        Some(revoked.cookie.as_str()),
    ] {
        let mut socket = connect(&server, BROWSER_PATH, Some(&server.origin()), cookie)
            .await
            .unwrap();
        // Without a hello: the browser goes to the sign-in page rather than reconnecting.
        match frame(&mut socket).await {
            Some(Message::Close(Some(frame))) => {
                assert_eq!(u16::from(frame.code), close::NOT_SIGNED_IN, "{cookie:?}");
            }
            other => panic!("expected a close for {cookie:?}, got {other:?}"),
        }
    }
    server.shut_down().await.unwrap();
}

#[tokio::test]
async fn revoking_a_sign_in_closes_its_browser_connection_on_every_instance() {
    let (_postgres, url) = postgres().await;
    let first = ready(&url, |_| {}).await;
    let second = ready(&url, |_| {}).await;
    let db = db::connect(&url).await.unwrap();
    let ann = create_user(&db).await;
    let laptop = sign_in(&db, ann, TimeDelta::hours(1)).await;
    let phone = sign_in(&db, ann, TimeDelta::hours(1)).await;
    let mut on_laptop = browser_of(&first, &laptop).await;
    let mut on_phone = browser_of(&second, &phone).await;
    let mut someone_else = browser(&first, &url).await;

    let txn = db.begin().await.unwrap();
    let revoked = sign_in_session::revoke(
        &txn,
        ann,
        Revoke::One(phone.session_id),
        Action::SignInSessionRevoked,
        Actor::System,
        Uuid::new_v4(),
    )
    .await
    .unwrap();
    txn.commit().await.unwrap();

    assert_eq!(revoked, [phone.session_id]);
    assert_eq!(closed_with(&mut on_phone).await, Some(close::NOT_SIGNED_IN));
    // The user's other browsers reload their list of sign-ins; other users' browsers hear nothing.
    assert_eq!(
        next_but_heartbeats(&mut on_laptop).await,
        ServerMessage::Event(Event {
            name: sign_in_session::CHANGED_EVENT.to_owned(),
            data: json!({}),
        })
    );
    realtime::publish(&db, Topic::Browsers, &event("for.browsers"))
        .await
        .unwrap();
    for socket in [&mut on_laptop, &mut someone_else] {
        assert_eq!(
            next_but_heartbeats(socket).await,
            ServerMessage::Event(event("for.browsers"))
        );
    }
    first.shut_down().await.unwrap();
    second.shut_down().await.unwrap();
}

#[tokio::test]
async fn browser_connections_close_when_their_sign_in_expires() {
    let (_postgres, url) = postgres().await;
    let server = ready(&url, |_| {}).await;
    let db = db::connect(&url).await.unwrap();
    let ann = create_user(&db).await;
    let started = Instant::now();
    let laptop = sign_in(&db, ann, TimeDelta::seconds(2)).await;
    let mut socket = browser_of(&server, &laptop).await;

    // Used in another tab in the meantime, which moves the end on.
    let mut session = sign_in_session::Entity::find_by_id(laptop.session_id)
        .one(&db)
        .await
        .unwrap()
        .unwrap()
        .into_active_model();
    session.expires_at = Set(Utc::now() + TimeDelta::seconds(3));
    session.update(&db).await.unwrap();

    assert_eq!(closed_with(&mut socket).await, Some(close::NOT_SIGNED_IN));
    let elapsed = started.elapsed();
    assert!(elapsed >= Duration::from_millis(2900), "{elapsed:?}");
    assert!(elapsed < Duration::from_secs(5), "{elapsed:?}");
    server.shut_down().await.unwrap();
}

#[tokio::test]
async fn shutdown_closes_connections_for_a_restart_without_waiting_for_them() {
    let (_postgres, url) = postgres().await;
    let server = ready(&url, |config| {
        config.http.shutdown_timeout = Duration::from_secs(30);
    })
    .await;
    let mut browser = browser(&server, &url).await;
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
