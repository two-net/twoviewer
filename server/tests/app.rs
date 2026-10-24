//! The server lifecycle against a real PostgreSQL: readiness tied to migrations and the database
//! (MNT-02, DB-05), and stopping without serving when migrations fail (DB-06). Needs Docker.

mod common;

use std::{
    net::SocketAddr,
    time::{Duration, Instant},
};

use common::{LOCK_TIMEOUT, execute, postgres};
use sqlx::{Connection, PgConnection};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    sync::oneshot,
    task::JoinHandle,
};
use twoviewer_server::{
    app::{self, RunError},
    config::{Config, DatabaseUrl, Secrets},
    db::migrate::{LOCK_KEY, MigrationError},
};

/// A server started with [`start`].
struct Server {
    monitoring: SocketAddr,
    stop: oneshot::Sender<()>,
    task: JoinHandle<Result<(), RunError>>,
}

/// Starts the server against `url` on free local ports.
async fn start(url: &str) -> Server {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let monitoring = listener.local_addr().unwrap();
    let mut config = Config::default();
    config.http.listen = "127.0.0.1:0".parse().unwrap();
    config.monitoring.readiness_timeout = Duration::from_millis(500);
    config.database.migration_lock_timeout = LOCK_TIMEOUT;
    let secrets = Secrets {
        database_url: DatabaseUrl::new(url),
    };
    let (stop, stopped) = oneshot::channel::<()>();
    let task = tokio::spawn(app::run(config, secrets, listener, async {
        let _ = stopped.await;
    }));
    Server {
        monitoring,
        stop,
        task,
    }
}

impl Server {
    /// The status and body of `GET path` on the monitoring listener.
    async fn get(&self, path: &str) -> (u16, String) {
        let mut stream = TcpStream::connect(self.monitoring).await.unwrap();
        let request =
            format!("GET {path} HTTP/1.1\r\nHost: localhost\r\nConnection: close\r\n\r\n");
        stream.write_all(request.as_bytes()).await.unwrap();
        let mut response = String::new();
        stream.read_to_string(&mut response).await.unwrap();
        let (head, body) = response.split_once("\r\n\r\n").unwrap();
        let status = head.split(' ').nth(1).unwrap().parse().unwrap();
        (status, body.to_owned())
    }

    /// Waits until `GET path` answers `status`, for at most 30 seconds.
    async fn wait_for(&self, path: &str, status: u16) {
        let deadline = Instant::now() + Duration::from_secs(30);
        while self.get(path).await.0 != status {
            assert!(Instant::now() < deadline, "{path} never answered {status}");
            tokio::time::sleep(Duration::from_millis(50)).await;
        }
    }

    /// Requests shutdown and returns the result of the run.
    async fn shut_down(self) -> Result<(), RunError> {
        let _ = self.stop.send(());
        tokio::time::timeout(Duration::from_secs(10), self.task)
            .await
            .expect("the server stops promptly")
            .unwrap()
    }
}

/// Waits until a session waits for an advisory lock.
async fn wait_for_lock_waiter(conn: &mut PgConnection) {
    let deadline = Instant::now() + Duration::from_secs(30);
    loop {
        let waiting: bool = sqlx::query_scalar(
            "SELECT EXISTS (SELECT FROM pg_locks WHERE locktype = 'advisory' AND NOT granted)",
        )
        .fetch_one(&mut *conn)
        .await
        .unwrap();
        if waiting {
            return;
        }
        assert!(
            Instant::now() < deadline,
            "the server never waited for the lock"
        );
        tokio::time::sleep(Duration::from_millis(50)).await;
    }
}

#[tokio::test]
async fn ready_only_once_migrations_have_completed() {
    let (_postgres, url) = postgres().await;
    // Another instance is migrating: it holds the migration lock (DB-07).
    let mut other = PgConnection::connect(&url).await.unwrap();
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(LOCK_KEY)
        .execute(&mut other)
        .await
        .unwrap();

    let server = start(&url).await;
    wait_for_lock_waiter(&mut other).await;

    // Alive while waiting for migrations, but not ready (DB-05).
    assert_eq!(server.get("/livez").await, (200, "alive".to_owned()));
    assert_eq!(server.get("/readyz").await, (503, "not ready".to_owned()));

    sqlx::query("SELECT pg_advisory_unlock($1)")
        .bind(LOCK_KEY)
        .execute(&mut other)
        .await
        .unwrap();
    server.wait_for("/readyz", 200).await;

    // MNT-04: the migration duration includes the wait for the other instance.
    let (status, metrics) = server.get("/metrics").await;
    assert_eq!(status, 200);
    let seconds: f64 = metrics
        .lines()
        .find_map(|line| line.strip_prefix("twoviewer_migration_duration_seconds "))
        .expect(&metrics)
        .parse()
        .unwrap();
    assert!(seconds > 0.0, "{metrics}");

    server.shut_down().await.unwrap();
}

#[tokio::test]
async fn not_ready_while_the_database_is_unreachable() {
    let (postgres, url) = postgres().await;
    let server = start(&url).await;
    server.wait_for("/readyz", 200).await;

    postgres.stop_with_timeout(Some(0)).await.unwrap();

    server.wait_for("/readyz", 503).await;
    assert_eq!(server.get("/livez").await.0, 200);
    server.shut_down().await.unwrap();
}

#[tokio::test]
async fn failed_migrations_stop_the_server() {
    let (_postgres, url) = postgres().await;
    // Not created by migrations, so the server refuses to migrate it.
    execute(&url, "CREATE TABLE stray (id integer)").await;

    let server = start(&url).await;

    let result = tokio::time::timeout(Duration::from_secs(30), server.task)
        .await
        .unwrap()
        .unwrap();
    assert!(
        matches!(
            result,
            Err(RunError::Migration(MigrationError::NotEmptyWithoutHistory))
        ),
        "{result:?}"
    );
}

#[tokio::test]
async fn shutdown_while_waiting_for_migrations_stops_promptly() {
    let (_postgres, url) = postgres().await;
    let mut other = PgConnection::connect(&url).await.unwrap();
    sqlx::query("SELECT pg_advisory_lock($1)")
        .bind(LOCK_KEY)
        .execute(&mut other)
        .await
        .unwrap();
    let server = start(&url).await;
    wait_for_lock_waiter(&mut other).await;

    server.shut_down().await.unwrap();
}
