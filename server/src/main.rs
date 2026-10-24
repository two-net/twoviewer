use std::process::ExitCode;

use tracing::{error, info};
use tracing_subscriber::EnvFilter;
use twoviewer_server::db;

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(
            EnvFilter::try_from_default_env().unwrap_or_else(|_| EnvFilter::new("info")),
        )
        .init();

    let Ok(database_url) = std::env::var("DATABASE_URL") else {
        error!("DATABASE_URL is not set");
        return ExitCode::FAILURE;
    };

    // DB-05: the schema is current before anything serves traffic. On failure, exit without
    // serving (DB-06).
    match db::migrate::migrate(&database_url).await {
        Ok(report) => info!(
            applied = ?report.applied.iter().map(|m| &m.id).collect::<Vec<_>>(),
            elapsed_ms = report.elapsed.as_millis(),
            version = env!("CARGO_PKG_VERSION"),
            "database schema is current"
        ),
        Err(err) => {
            error!(error = %err, "database migration failed; exiting");
            return ExitCode::FAILURE;
        }
    }

    let db = match db::connect(&database_url).await {
        Ok(db) => db,
        Err(err) => {
            error!(error = %err, "cannot connect to the database; exiting");
            return ExitCode::FAILURE;
        }
    };

    // #12 starts the HTTP listener here and reports ready only from this point (MNT-02).
    info!("started");
    shutdown_signal().await;
    info!("shutting down");
    let _ = db.close().await;
    ExitCode::SUCCESS
}

async fn shutdown_signal() {
    let ctrl_c = tokio::signal::ctrl_c();
    #[cfg(unix)]
    {
        let mut terminate =
            tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
                .expect("install SIGTERM handler");
        tokio::select! {
            _ = ctrl_c => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = ctrl_c.await;
}
