//! The server application's lifecycle: start, serve and shut down.
//!
//! 1. The monitoring listener serves `/livez`, `/readyz` and `/metrics` from the start. Until
//!    step 3, `/readyz` reports not ready (MNT-02).
//! 2. Email is set up, without connecting to the email delivery service, so that its outages
//!    never keep the server from starting or from being ready. The TLS certificate is loaded,
//!    pending migrations are applied, then the connection pool opens (DB-05) and the server starts
//!    receiving real-time events. If any of these fails, the server stops without having served
//!    traffic (DB-06).
//! 3. The public listeners open: HTTPS (IF-08), and plain HTTP, which only redirects to HTTPS
//!    (SEC-01). `/readyz` reports ready while the database answers.
//! 4. On shutdown, `/readyz` reports not ready, so that load balancers stop sending traffic, and
//!    real-time connections are closed, so that their clients reconnect elsewhere. Open requests
//!    and connections get `http.shutdown_timeout` to finish, and after them the emails being
//!    sent. Emails waiting to be tried again are abandoned.

use std::{future::Future, io, net::SocketAddr, pin::pin, sync::Arc};

use axum::serve::{Listener as _, ListenerExt as _};
use sea_orm::{DatabaseConnection, DbErr};
use tokio::{net::TcpListener, sync::oneshot};
use tracing::{info, warn};

use crate::{
    accounts::{Accounts, password::Passwords},
    config::{Config, Secrets},
    db::{self, migrate::MigrationError},
    email::{self, Mailer},
    http,
    metrics::Metrics,
    monitoring::{self, Health},
    realtime::Realtime,
    redirect,
    tls::{self, Certificate, TlsError, TlsListener},
};

#[derive(Debug, thiserror::Error)]
pub enum RunError {
    #[error("database migration failed: {0}")]
    Migration(#[from] MigrationError),
    #[error("cannot connect to the database: {0}")]
    Database(#[from] DbErr),
    #[error("cannot receive real-time events from the database: {0}")]
    Realtime(#[source] sqlx::Error),
    #[error("cannot load the TLS certificate: {0}")]
    Tls(#[from] TlsError),
    #[error("cannot set up email: {0}")]
    Email(#[from] email::SetupError),
    #[error(transparent)]
    Config(#[from] crate::config::ConfigError),
    #[error("cannot set up password hashing: {0}")]
    Passwords(argon2::Error),
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
    // Only now, so that `/readyz` reports not ready for as long as the HTTPS listener drains.
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
    // First, so that settings that cannot be used stop the server before it migrates.
    let mailer = Mailer::new(
        &config.email,
        secrets.smtp_credentials.as_ref(),
        metrics.clone(),
    )?;
    let passwords = Passwords::new(&config.passwords).map_err(RunError::Passwords)?;
    let public_url = config.http.public_url()?;
    let certificate = Arc::new(Certificate::load(&config.tls).await?);
    let mut shutdown = Box::pin(shutdown);
    let (db, realtime) = tokio::select! {
        started = start(config, secrets, metrics) => started?,
        () = &mut shutdown => {
            // Abandoning the run closes its connection, so PostgreSQL rolls back an unfinished
            // migration and releases the lock (DB-06, DB-07).
            info!("shutting down before startup completed");
            return Ok(());
        }
    };

    let address = config.http.listen;
    let listener = bind(address).await?;
    let redirect_listener = bind(config.http.redirect_listen).await?;
    let listener = TlsListener::new(
        listener,
        tls::server_config(certificate.clone()),
        config.tls.handshake_timeout,
    )
    .map_err(|source| RunError::Listen { address, source })?;
    health.migrated(db.clone());
    info!(
        address = %listener.local_addr().unwrap_or(address),
        redirect_address = %redirect_listener
            .local_addr()
            .unwrap_or(config.http.redirect_listen),
        version = env!("CARGO_PKG_VERSION"),
        "started"
    );
    // Redirects need no draining, and neither does reloading the certificate: both stop once
    // the HTTPS listener has drained.
    let redirects = tokio::spawn(
        axum::serve(redirect_listener, redirect::router(config.http.public_port)).into_future(),
    );
    let reload = {
        let interval = config.tls.reload_interval;
        tokio::spawn(async move { certificate.reload(interval).await })
    };

    let (draining, drain_started) = oneshot::channel();
    let signal = {
        let health = health.clone();
        let realtime = realtime.clone();
        async move {
            shutdown.await;
            info!("shutting down");
            health.shutting_down();
            realtime.close_all();
            let _ = draining.send(());
        }
    };
    let accounts = Accounts::new(
        db.clone(),
        mailer.clone(),
        passwords,
        public_url,
        &config.accounts,
        &config.sign_up,
    );
    let routes = realtime.routes().merge(accounts.routes());
    // With the address of each connection, which audit events record (`http::ClientOrigin`).
    // axum provides it for its own listeners, and for others once wrapped with `tap_io`.
    let server = axum::serve(
        listener.tap_io(|_| {}),
        http::router(routes, metrics.clone(), config.http.hsts_max_age)
            .into_make_service_with_connect_info::<SocketAddr>(),
    )
    .with_graceful_shutdown(signal)
    .into_future();
    // Only completes after the shutdown signal. axum waits for open requests, but not for
    // real-time connections, which it hands over once upgraded.
    let drained = pin!(async {
        // It never returns an error.
        let _ = server.await;
        realtime.wait_closed().await;
        // Only now, since requests send emails until they have finished.
        mailer.shut_down().await;
    });
    let timeout = async {
        let _ = drain_started.await;
        tokio::time::sleep(config.http.shutdown_timeout).await;
    };
    tokio::select! {
        biased;
        () = drained => {}
        () = timeout => warn!("open connections did not finish within the shutdown timeout"),
    }

    redirects.abort();
    reload.abort();
    let _ = db.close().await;
    Ok(())
}

async fn bind(address: SocketAddr) -> Result<TcpListener, RunError> {
    TcpListener::bind(address)
        .await
        .map_err(|source| RunError::Listen { address, source })
}

/// Applies pending migrations, then opens the connection pool and starts receiving real-time
/// events.
async fn start(
    config: &Config,
    secrets: &Secrets,
    metrics: &Arc<Metrics>,
) -> Result<(DatabaseConnection, Arc<Realtime>), RunError> {
    let database_url = secrets.database_url.expose();
    let report = db::migrate::migrate(database_url, config.database.migration_lock_timeout).await?;
    metrics.migration_duration.set(report.elapsed.as_secs_f64());
    info!(
        applied = ?report.applied.iter().map(|m| &m.id).collect::<Vec<_>>(),
        elapsed_ms = report.elapsed.as_millis() as u64,
        version = env!("CARGO_PKG_VERSION"),
        "database schema is current"
    );
    let db = db::connect(database_url).await?;
    let realtime = Realtime::start(database_url, config.realtime.clone(), metrics.clone())
        .await
        .map_err(RunError::Realtime)?;
    Ok((db, realtime))
}
