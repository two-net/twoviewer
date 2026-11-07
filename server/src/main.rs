use std::{process::ExitCode, sync::Arc};

use tokio::net::TcpListener;
use tracing::{error, info};
use twoviewer_server::{
    app,
    config::{Config, Secrets, SmtpCredentials},
    db::EmailAddress,
    email::{Mailer, TestEmail},
    metrics::Metrics,
    telemetry,
};

const USAGE: &str = "usage: twoviewer-server
           Runs the server.
       twoviewer-server send-test-email <address>
           Sends a test email to <address> with the server's email settings, to check email
           delivery and sender authentication.";

#[tokio::main]
async fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    match args.iter().map(String::as_str).collect::<Vec<_>>()[..] {
        [] => serve().await,
        ["send-test-email", address] => send_test_email(address).await,
        _ => {
            eprintln!("{USAGE}");
            ExitCode::FAILURE
        }
    }
}

async fn serve() -> ExitCode {
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

/// Sends [`TestEmail`] to `address` once, and prints the SMTP server's reply. It needs the
/// server's configuration and SMTP credentials, but not the database.
async fn send_test_email(address: &str) -> ExitCode {
    let mailer = Config::load()
        .and_then(|config| Ok((config, SmtpCredentials::from_env()?)))
        .map_err(|err| err.to_string())
        .and_then(|(config, credentials)| {
            Mailer::new(
                &config.email,
                credentials.as_ref(),
                Arc::new(Metrics::new()),
            )
            .map_err(|err| err.to_string())
        });
    let mailer = match mailer {
        Ok(mailer) => mailer,
        Err(err) => {
            eprintln!("error: {err}");
            return ExitCode::FAILURE;
        }
    };
    match mailer
        .send_now(&EmailAddress::new(address), &TestEmail)
        .await
    {
        Ok(response) => {
            println!(
                "Sent. The SMTP server replied {}: {}",
                response.code(),
                response.message().collect::<Vec<_>>().join(" ")
            );
            ExitCode::SUCCESS
        }
        Err(err) => {
            eprintln!("error: the test email was not sent: {err}");
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
