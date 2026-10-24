//! Operational logs (MNT-03): structured, on standard output, and separate from audit events.
//!
//! Requests on the public listener log inside a span that carries their correlation ID
//! ([`crate::http`]), so every line written while handling a request can be traced back to it.
//! Logs never contain secrets or session content: log identifiers and route templates, not
//! credentials, request paths, query strings, headers or bodies. Record numbers as `u64` or
//! smaller: larger integers end up as strings in JSON.

use tracing::{Subscriber, subscriber::SetGlobalDefaultError};
use tracing_subscriber::{EnvFilter, filter::ParseError, fmt::MakeWriter};

use crate::config::{Log, LogFormat};

#[derive(Debug, thiserror::Error)]
pub enum InitError {
    #[error("invalid log.filter: {0}")]
    Filter(#[from] ParseError),
    #[error(transparent)]
    AlreadySet(#[from] SetGlobalDefaultError),
}

/// Writes the logs of the whole process to standard output, as configured.
pub fn init(config: &Log) -> Result<(), InitError> {
    tracing::subscriber::set_global_default(subscriber(config, std::io::stdout)?)?;
    Ok(())
}

/// A subscriber that writes logs as configured to `writer`.
pub fn subscriber<W>(
    config: &Log,
    writer: W,
) -> Result<Box<dyn Subscriber + Send + Sync>, ParseError>
where
    W: for<'w> MakeWriter<'w> + Send + Sync + 'static,
{
    let builder = tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_new(&config.filter)?)
        .with_writer(writer);
    Ok(match config.format {
        // The fields of the current span, such as the correlation ID, go on every line.
        LogFormat::Json => Box::new(
            builder
                .json()
                .with_current_span(true)
                .with_span_list(false)
                .finish(),
        ),
        LogFormat::Text => Box::new(builder.finish()),
    })
}
