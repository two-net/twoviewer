//! Connects to the server named by `TWOVIEWER_SERVER`, such as `https://twoviewer.example`, and
//! stays connected until stopped with Ctrl+C. Logs go to standard error; `RUST_LOG` filters them.

use std::process::ExitCode;

use tokio::sync::{mpsc, watch};
use tracing::{debug, error, info};
use tracing_subscriber::EnvFilter;
use twoviewer_agent::connection::{self, Connection, State};

#[tokio::main]
async fn main() -> ExitCode {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .with_writer(std::io::stderr)
        .init();

    let Some(server) = std::env::var("TWOVIEWER_SERVER")
        .ok()
        .filter(|server| !server.is_empty())
    else {
        error!(
            "TWOVIEWER_SERVER is not set: set it to the server's address, such as https://twoviewer.example"
        );
        return ExitCode::FAILURE;
    };
    let connection = match connection::system_tls()
        .map_err(|err| err.to_string())
        .and_then(|tls| Connection::new(&server, tls).map_err(|err| err.to_string()))
    {
        Ok(connection) => connection,
        Err(err) => {
            error!(error = %err, "cannot connect");
            return ExitCode::FAILURE;
        }
    };

    let (received_tx, mut received) = mpsc::unbounded_channel();
    let (state_tx, _state) = watch::channel(State::Connecting);
    let mut running = std::pin::pin!(connection.run(received_tx, state_tx));
    loop {
        tokio::select! {
            () = &mut running => unreachable!("the connection is kept until the agent stops"),
            _ = tokio::signal::ctrl_c() => {
                info!("stopping");
                return ExitCode::SUCCESS;
            }
            Some(message) = received.recv() => debug!(?message, "received"),
        }
    }
}
