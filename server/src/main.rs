use std::process::ExitCode;

use tokio::net::TcpListener;
use tracing::{error, info};
use twoviewer_server::{
    app,
    config::{Config, Secrets},
    telemetry,
};

#[tokio::main]
async fn main() -> ExitCode {
    // Logging is set up from the configuration, so errors until then go to standard error.
    let (config, secrets) = match Config::load().and_then(|c| Ok((c, Secrets::from_env()?))) {
        Ok(loaded) => loaded,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };
    if let Err(err) = telemetry::init(&config.log) {
        eprintln!("error: {err}");
        return ExitCode::FAILURE;
    }
    // The configuration holds no secrets; they are in `secrets`.
    info!(?config, "configuration loaded");

    let monitoring = match TcpListener::bind(config.monitoring.listen).await {
        Ok(listener) => listener,
        Err(err) => {
            error!(address = %config.monitoring.listen, error = %err, "cannot listen; exiting");
            return ExitCode::FAILURE;
        }
    };
    match app::run(config, secrets, monitoring, shutdown_signal()).await {
        Ok(()) => {
            info!("stopped");
            ExitCode::SUCCESS
        }
        Err(err) => {
            error!(error = %err, "exiting");
            ExitCode::FAILURE
        }
    }
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
