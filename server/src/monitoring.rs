//! The monitoring listener: liveness and readiness checks (MNT-02) and metrics (MNT-04).
//!
//! It opens as soon as the server starts, before migrations, and stays private. Its requests are
//! not logged, because probes and scrapes arrive every few seconds.

use std::{
    sync::{
        Arc, OnceLock,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};

use axum::{
    Router,
    extract::State,
    http::{StatusCode, header},
    response::IntoResponse,
    routing::get,
};
use sea_orm::DatabaseConnection;
use tracing::warn;

use crate::metrics::{self, Metrics};

/// What the readiness check reports on.
pub struct Health {
    /// Set once migrations have completed and the connection pool is open (DB-05).
    db: OnceLock<DatabaseConnection>,
    shutting_down: AtomicBool,
    readiness_timeout: Duration,
}

impl Health {
    pub fn new(readiness_timeout: Duration) -> Self {
        Self {
            db: OnceLock::new(),
            shutting_down: AtomicBool::new(false),
            readiness_timeout,
        }
    }

    /// Records that migrations have completed and `db` is open. From now on the server is ready
    /// while the database answers.
    pub fn migrated(&self, db: DatabaseConnection) {
        let _ = self.db.set(db);
    }

    /// Records that the server is shutting down, so that load balancers stop sending it traffic.
    pub fn shutting_down(&self) {
        self.shutting_down.store(true, Ordering::Relaxed);
    }

    /// Ready means migrated, not shutting down, and the database answers within the timeout.
    pub async fn is_ready(&self) -> bool {
        if self.shutting_down.load(Ordering::Relaxed) {
            return false;
        }
        let Some(db) = self.db.get() else {
            return false;
        };
        match tokio::time::timeout(self.readiness_timeout, db.ping()).await {
            Ok(Ok(())) => true,
            Ok(Err(err)) => {
                warn!(error = %err, "not ready: the database does not answer");
                false
            }
            Err(_) => {
                warn!("not ready: the database did not answer in time");
                false
            }
        }
    }
}

#[derive(Clone)]
struct MonitoringState {
    health: Arc<Health>,
    metrics: Arc<Metrics>,
}

/// `/livez`, `/readyz` and `/metrics`.
pub fn router(health: Arc<Health>, metrics: Arc<Metrics>) -> Router {
    Router::new()
        .route("/livez", get(livez))
        .route("/readyz", get(readyz))
        .route("/metrics", get(scrape))
        .with_state(MonitoringState { health, metrics })
}

/// Alive while the process serves requests. It does not depend on the database, so a database
/// outage makes the server unready rather than restarted.
async fn livez() -> &'static str {
    "alive"
}

async fn readyz(State(state): State<MonitoringState>) -> impl IntoResponse {
    if state.health.is_ready().await {
        (StatusCode::OK, "ready")
    } else {
        (StatusCode::SERVICE_UNAVAILABLE, "not ready")
    }
}

async fn scrape(State(state): State<MonitoringState>) -> impl IntoResponse {
    (
        [(header::CONTENT_TYPE, metrics::CONTENT_TYPE)],
        state.metrics.encode(),
    )
}
