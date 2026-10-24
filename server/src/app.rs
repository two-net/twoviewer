//! The server application's lifecycle: start, serve and shut down.
//!
//! 1. The monitoring listener serves `/livez`, `/readyz` and `/metrics` from the start. Until
//!    step 3, `/readyz` reports not ready (MNT-02).
//! 2. Pending migrations are applied, then the connection pool opens (DB-05). If either fails,
//!    the server stops without having served traffic (DB-06).
//! 3. The public listener opens, and `/readyz` reports ready while the database answers.
//! 4. On shutdown, `/readyz` reports not ready, so that load balancers stop sending traffic, and
//!    open requests get `http.shutdown_timeout` to finish.

use std::{future::Future, io, net::SocketAddr, pin::pin, sync::Arc};

use sea_orm::{DatabaseConnection, DbErr};
use tokio::{net::TcpListener, sync::oneshot};
use tracing::{info, warn};

use crate::{
    config::{Config, Secrets},
    db::{self, migrate::MigrationError},
    http,
    metrics::Metrics,
    monitoring::{self, Health},
};

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("database migration failed: {0}")]
    Migration(#[from] MigrationError),
    #[error("cannot connect to the database: {0}")]
    Database(#[from] DbErr),
    #[error("cannot listen on {address}: {source}")]
    Listen {
        address: SocketAddr,
        #[source]
        source: io::Error,
    },
}

/// Runs the server until `shutdown` completes.
///
/// The caller binds `monitoring`, the listener for `config.monitoring.listen`, so that it is open
/// before anything else happens.
pub async fn run(
    config: Config,
    secrets: Secrets,
    monitoring: TcpListener,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), RunError> {
    let metrics = Arc::new(Metrics::new());
    let health = Arc::new(Health::new(config.monitoring.readiness_timeout));
    if let Ok(address) = monitoring.local_addr() {
        info!(%address, "monitoring listener open");
    }
    let monitoring = tokio::spawn(
        axum::serve(
            monitoring,
            monitoring::router(health.clone(), metrics.clone()),
        )
        .into_future(),
    );

    let result = serve(&config, &secrets, &health, &metrics, shutdown).await;
    // Only now, so that `/readyz` reports not ready for as long as the public listener drains.
    monitoring.abort();
    result
}

async fn serve(
    config: &Config,
    secrets: &Secrets,
    health: &Arc<Health>,
    metrics: &Arc<Metrics>,
    shutdown: impl Future<Output = ()> + Send + 'static,
) -> Result<(), RunError> {
    let mut shutdown = Box::pin(shutdown);
    let db = tokio::select! {
        db = start(config, secrets, metrics) => db?,
        () = &mut shutdown => {
            // Abandoning the run closes its connection, so PostgreSQL rolls back an unfinished
            // migration and releases the lock (DB-06, DB-07).
            info!("shutting down before startup completed");
            return Ok(());
        }
    };

    let address = config.http.listen;
    let listener = TcpListener::bind(address)
        .await
        .map_err(|source| RunError::Listen { address, source })?;
    health.migrated(db.clone());
    info!(
        address = %listener.local_addr().unwrap_or(address),
        version = env!("CARGO_PKG_VERSION"),
        "started"
    );

    let (draining, drain_started) = oneshot::channel();
    let signal = {
        let health = health.clone();
        async move {
            shutdown.await;
            info!("shutting down");
            health.shutting_down();
            let _ = draining.send(());
        }
    };
    let server = pin!(
        axum::serve(listener, http::router(metrics.clone()))
            .with_graceful_shutdown(signal)
            .into_future()
    );
    let timeout = async {
        let _ = drain_started.await;
        tokio::time::sleep(config.http.shutdown_timeout).await;
    };
    tokio::select! {
        biased;
        // Only completes after the shutdown signal, once open connections are closed. It never
        // returns an error.
        _ = server => {}
        () = timeout => warn!("open connections did not finish within the shutdown timeout"),
    }

    let _ = db.close().await;
    Ok(())
}

/// Applies pending migrations, then opens the connection pool.
async fn start(
    config: &Config,
    secrets: &Secrets,
    metrics: &Metrics,
) -> Result<DatabaseConnection, RunError> {
    let database_url = secrets.database_url.expose();
    let report = db::migrate::migrate(database_url, config.database.migration_lock_timeout).await?;
    metrics.migration_duration.set(report.elapsed.as_secs_f64());
    info!(
        applied = ?report.applied.iter().map(|m| &m.id).collect::<Vec<_>>(),
        elapsed_ms = report.elapsed.as_millis() as u64,
        version = env!("CARGO_PKG_VERSION"),
        "database schema is current"
    );
    Ok(db::connect(database_url).await?)
}
