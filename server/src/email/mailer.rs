//! Delivery: a queue in memory, retries, and shutdown.

use std::{sync::Arc, time::Duration};

use lettre::{
    Address, AsyncSmtpTransport, AsyncTransport, Tokio1Executor,
    address::{AddressError, Envelope},
    transport::smtp::{
        self, PoolConfig,
        authentication::Credentials,
        client::{Tls, TlsParameters},
        extension::ClientId,
        response::Response,
    },
};
use tokio::sync::Semaphore;
use tokio_util::{sync::CancellationToken, task::TaskTracker};
use tracing::{Instrument, Span, info, warn};

use super::{Email, message};
use crate::{
    config::{self, ConfigError, SmtpCredentials, SmtpSecurity},
    db::EmailAddress,
    metrics::Metrics,
};

/// Sends emails through the email delivery service. Clones share the same queue.
#[derive(Clone)]
pub struct Mailer {
    inner: Arc<Inner>,
}

struct Inner {
    transport: AsyncSmtpTransport<Tokio1Executor>,
    from: Address,
    max_attempts: u32,
    retry_delay: Duration,
    /// A permit for each email that may wait to be sent (`email.max_pending`).
    pending: Arc<Semaphore>,
    deliveries: TaskTracker,
    /// Cancelled at shutdown: no more emails are accepted, and those waiting to be tried again are
    /// abandoned.
    stopping: CancellationToken,
    metrics: Arc<Metrics>,
}

/// Why the mailer cannot be set up.
#[derive(Debug, thiserror::Error)]
pub enum SetupError {
    #[error(transparent)]
    Config(#[from] ConfigError),
    #[error("cannot set up TLS for the SMTP server: {0}")]
    Tls(#[source] smtp::Error),
}

/// Why an email was not sent.
#[derive(Debug, thiserror::Error)]
pub enum SendError {
    /// The address is not shown, since it is personal data.
    #[error("the recipient is not an email address")]
    Recipient(#[source] AddressError),
    #[error("cannot render the email: {0}")]
    Render(#[from] askama::Error),
    #[error("cannot build the email: {0}")]
    Build(#[from] lettre::error::Error),
    /// Its text can include the SMTP server's reply, which can repeat the recipient's address.
    #[error("{0}")]
    Smtp(#[from] smtp::Error),
}

/// An email ready to be sent.
struct Outgoing {
    name: &'static str,
    /// Without the angle brackets. It is not personal, and it lets operators find the email in
    /// the logs of the email delivery service.
    message_id: String,
    envelope: Envelope,
    formatted: Vec<u8>,
}

impl Mailer {
    /// Sets up sending through the SMTP server in `config`. It connects only once there is an
    /// email to send, so it works while the email delivery service is unavailable.
    pub fn new(
        config: &config::Email,
        credentials: Option<&SmtpCredentials>,
        metrics: Arc<Metrics>,
    ) -> Result<Self, SetupError> {
        let (from, host) = config.server()?;
        let tls_parameters = || TlsParameters::new(host.to_owned()).map_err(SetupError::Tls);
        let tls = match config.smtp_security {
            SmtpSecurity::Starttls => Tls::Required(tls_parameters()?),
            SmtpSecurity::Tls => Tls::Wrapper(tls_parameters()?),
            SmtpSecurity::None if credentials.is_some() => {
                return Err(ConfigError::CredentialsInClear.into());
            }
            SmtpSecurity::None => Tls::None,
        };
        let mut transport = AsyncSmtpTransport::<Tokio1Executor>::builder_dangerous(host)
            .port(config.smtp_port)
            .tls(tls)
            .timeout(Some(config.timeout))
            .pool_config(PoolConfig::new().max_size(config.max_connections))
            // The sender's domain rather than this machine's name, which says nothing to the
            // SMTP server and would otherwise be used.
            .hello_name(ClientId::Domain(from.domain().to_owned()));
        if let Some(credentials) = credentials {
            transport = transport.credentials(Credentials::new(
                credentials.username().to_owned(),
                credentials.expose_password().to_owned(),
            ));
        }
        Ok(Self {
            inner: Arc::new(Inner {
                transport: transport.build(),
                from,
                max_attempts: config.max_attempts,
                retry_delay: config.retry_delay,
                pending: Arc::new(Semaphore::new(config.max_pending)),
                deliveries: TaskTracker::new(),
                stopping: CancellationToken::new(),
                metrics,
            }),
        })
    }

    /// Queues `email` to `to` and returns at once. It is delivered in the background: emails
    /// that fail for a temporary reason are tried again up to `email.max_attempts` times. If
    /// `email.max_pending` emails are already waiting, or the server is shutting down, it is
    /// dropped.
    ///
    /// Whether it is sent shows only in logs and in the metrics `twoviewer_emails_sent_total`
    /// and `twoviewer_emails_failed_total`.
    pub fn send<E: Email>(&self, to: &EmailAddress, email: &E) {
        let inner = &self.inner;
        let outgoing = match inner.prepare(to, email) {
            Ok(outgoing) => outgoing,
            Err(err) => {
                warn!(email = E::NAME, error = %err, "email not sent: it cannot be built");
                inner.metrics.emails_failed.inc();
                return;
            }
        };
        let message_id = outgoing.message_id.as_str();
        if inner.stopping.is_cancelled() {
            warn!(
                email = E::NAME,
                message_id, "email dropped: the server is shutting down"
            );
            inner.metrics.emails_failed.inc();
            return;
        }
        let Ok(permit) = inner.pending.clone().try_acquire_owned() else {
            warn!(
                email = E::NAME,
                message_id, "email dropped: email.max_pending emails are already waiting"
            );
            inner.metrics.emails_failed.inc();
            return;
        };
        let task = {
            let inner = inner.clone();
            async move {
                inner.deliver(&outgoing).await;
                drop(permit);
            }
        };
        // With the current span, so that the logs carry the correlation ID of the request that
        // sent the email.
        inner.deliveries.spawn(task.instrument(Span::current()));
    }

    /// Sends `email` to `to` right away, once, and returns the SMTP server's reply. For
    /// `twoviewer-server send-test-email`: features use [`Mailer::send`].
    pub async fn send_now<E: Email>(
        &self,
        to: &EmailAddress,
        email: &E,
    ) -> Result<Response, SendError> {
        let outgoing = self.inner.prepare(to, email)?;
        let response = self
            .inner
            .transport
            .send_raw(&outgoing.envelope, &outgoing.formatted)
            .await?;
        Ok(response)
    }

    /// Stops accepting emails, abandons those waiting to be tried again, and waits for the
    /// attempts in progress, which take at most `email.timeout` per SMTP command.
    pub async fn shut_down(&self) {
        self.inner.stopping.cancel();
        self.inner.deliveries.close();
        self.inner.deliveries.wait().await;
    }
}

impl Inner {
    fn prepare<E: Email>(&self, to: &EmailAddress, email: &E) -> Result<Outgoing, SendError> {
        let to = to.as_str().parse().map_err(SendError::Recipient)?;
        let (message, message_id) = message::build(&self.from, to, email.render()?)?;
        Ok(Outgoing {
            name: E::NAME,
            message_id,
            envelope: message.envelope().clone(),
            formatted: message.formatted(),
        })
    }

    async fn deliver(&self, outgoing: &Outgoing) {
        let (email, message_id) = (outgoing.name, outgoing.message_id.as_str());
        let mut delay = self.retry_delay;
        for attempt in 1..=self.max_attempts {
            let err = match self
                .transport
                .send_raw(&outgoing.envelope, &outgoing.formatted)
                .await
            {
                Ok(_) => {
                    info!(email, message_id, attempt, "email sent");
                    self.metrics.emails_sent.inc();
                    return;
                }
                Err(err) => err,
            };
            let reason = describe(&err);
            if is_final(&err) {
                warn!(
                    email,
                    message_id, attempt, reason, "email not sent: rejected"
                );
                break;
            }
            if attempt == self.max_attempts {
                warn!(
                    email,
                    message_id, attempt, reason, "email not sent: failed on every attempt"
                );
                break;
            }
            warn!(email, message_id, attempt, reason, retry_in = ?delay, "email not sent yet");
            tokio::select! {
                () = tokio::time::sleep(delay) => {}
                () = self.stopping.cancelled() => {
                    warn!(email, message_id, "email not sent: abandoned at shutdown");
                    break;
                }
            }
            delay = delay.saturating_mul(2);
        }
        self.metrics.emails_failed.inc();
    }
}

/// Whether trying `err`'s email again is pointless: the SMTP server rejected it for good (a 5xx
/// reply), or the email itself is wrong. Anything else may pass, such as a 4xx reply, a refused
/// connection, a timeout or a failed TLS handshake.
fn is_final(err: &smtp::Error) -> bool {
    err.is_permanent() || err.is_client()
}

/// What went wrong, for logs. Without the SMTP server's reply text, which can repeat the
/// recipient's address.
fn describe(err: &smtp::Error) -> String {
    match err.status() {
        Some(code) => format!("the SMTP server replied {code}"),
        None if err.is_response() => "the SMTP server's reply cannot be read".to_owned(),
        None => err.to_string(),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn config() -> config::Email {
        config::Email {
            from_address: Some("no-reply@twoviewer.test".to_owned()),
            smtp_host: Some("localhost".to_owned()),
            ..config::Email::default()
        }
    }

    fn mailer(
        config: &config::Email,
        credentials: Option<&SmtpCredentials>,
    ) -> Result<Mailer, SetupError> {
        Mailer::new(config, credentials, Arc::new(Metrics::new()))
    }

    #[tokio::test]
    async fn needs_a_sender_and_a_server() {
        let config = config::Email::default();

        assert!(matches!(
            mailer(&config, None),
            Err(SetupError::Config(ConfigError::EmailServer))
        ));
    }

    #[tokio::test]
    async fn credentials_are_never_sent_unencrypted() {
        let credentials = SmtpCredentials::new("apikey", "hunter3");
        let mut config = config();

        for security in [SmtpSecurity::Starttls, SmtpSecurity::Tls] {
            config.smtp_security = security;
            assert!(mailer(&config, Some(&credentials)).is_ok(), "{security:?}");
        }
        config.smtp_security = SmtpSecurity::None;
        assert!(mailer(&config, None).is_ok());
        assert!(matches!(
            mailer(&config, Some(&credentials)),
            Err(SetupError::Config(ConfigError::CredentialsInClear))
        ));
    }
}
