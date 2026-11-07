//! Operator configuration (MNT-01) and runtime secrets (SEC-13).
//!
//! Every limit, timeout and other number the server uses is a setting with its default here, so
//! operators change it without code changes. Later sources override earlier ones:
//!
//! 1. the defaults in this module, which are the numbers in the SRS;
//! 2. the TOML file named by `TWOVIEWER_CONFIG`, if set;
//! 3. environment variables `TWOVIEWER_<SECTION>__<SETTING>`, such as
//!    `TWOVIEWER_DATABASE__MIGRATION_LOCK_TIMEOUT=30s`.
//!
//! `server/config.example.toml` lists every setting with its default. Unknown settings are
//! rejected, so a typo fails startup instead of being ignored. A new number goes into the section
//! of the feature that uses it, with a doc comment, and into the example file.
//!
//! Secrets are not settings. They come only from the environment at runtime ([`Secrets`]), never
//! from the file, which may well be kept in source control.
//!
//! A few settings have no default, such as the sender of emails, so the server does not start
//! until they are set.

use std::{fmt, net::SocketAddr, path::PathBuf, time::Duration};

use figment::{
    Figment,
    providers::{Env, Format, Serialized, Toml},
};
use lettre::Address;
use serde::{Deserialize, Serialize};

/// The environment variable that names the configuration file.
const FILE_VAR: &str = "TWOVIEWER_CONFIG";
const ENV_PREFIX: &str = "TWOVIEWER_";

#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub http: Http,
    pub tls: Tls,
    pub monitoring: Monitoring,
    pub database: Database,
    pub realtime: Realtime,
    pub email: Email,
    pub log: Log,
}

/// The public listeners: HTTPS for the API and the web client (§2.2), and plain HTTP, which only
/// redirects to HTTPS (SEC-01).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Http {
    /// Address of the HTTPS listener. It opens only once migrations have completed (DB-05).
    pub listen: SocketAddr,
    /// Address of the plain-HTTP listener, which redirects every request to HTTPS. It opens
    /// together with the HTTPS listener.
    pub redirect_listen: SocketAddr,
    /// The port clients reach the HTTPS listener on, where redirects point. It differs from the
    /// port in `listen` when a load balancer or container maps ports.
    pub public_port: u16,
    /// How long browsers remember to use only HTTPS for the site (HSTS, SEC-01), in whole seconds.
    /// 0 makes them forget it, which suits development on `localhost`.
    #[serde(with = "humantime_serde")]
    pub hsts_max_age: Duration,
    /// How long shutdown waits for open requests and connections before closing them.
    #[serde(with = "humantime_serde")]
    pub shutdown_timeout: Duration,
}

/// The certificate of the HTTPS listener (IF-08). It comes from PEM files, or for development
/// only, is generated at startup.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Tls {
    /// The certificate chain, leaf first.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub certificate_file: Option<PathBuf>,
    /// The private key of the certificate. The file is provided at runtime and never kept in
    /// source control (SEC-13).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub private_key_file: Option<PathBuf>,
    /// Instead of the files, a self-signed certificate for `localhost`, generated at startup.
    /// Browsers and Host Agents do not trust it: for development only.
    pub self_signed: bool,
    /// How often the files are checked for a renewed certificate.
    #[serde(with = "humantime_serde")]
    pub reload_interval: Duration,
    /// How long a client gets to complete the TLS handshake before it is disconnected.
    #[serde(with = "humantime_serde")]
    pub handshake_timeout: Duration,
}

/// The monitoring listener, for liveness, readiness and metrics (MNT-02, MNT-04). Keep it
/// private: it is not meant to be reachable from the internet.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Monitoring {
    /// Address of the monitoring listener. It opens as soon as the server starts.
    pub listen: SocketAddr,
    /// How long the readiness check waits for the database before reporting not ready.
    #[serde(with = "humantime_serde")]
    pub readiness_timeout: Duration,
}

#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Database {
    /// How long a migration waits for a table lock before it fails, so that a blocked migration
    /// cannot queue live traffic behind it (REL-02).
    #[serde(with = "humantime_serde")]
    pub migration_lock_timeout: Duration,
}

/// The real-time connection of browsers and Host Agents (IF-09).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Realtime {
    /// How often the server sends a heartbeat on each connection. Clients answer every one.
    #[serde(with = "humantime_serde")]
    pub heartbeat_interval: Duration,
    /// How long a connection may be silent before it counts as lost: the server closes it, and
    /// clients, which are told this value, reconnect. It must be longer than the interval. With
    /// the defaults, a Host Agent that disappears without closing its connection is offline
    /// within 10 seconds (DEV-07).
    #[serde(with = "humantime_serde")]
    pub heartbeat_timeout: Duration,
    /// The largest message a client may send, in bytes.
    pub max_message_bytes: usize,
    /// How many messages may wait to be sent on one connection. A client that falls this far
    /// behind is disconnected, and reconnects.
    pub send_queue_length: usize,
    /// How long to wait before retrying when the database connection that receives events from
    /// every instance cannot be reestablished.
    #[serde(with = "humantime_serde")]
    pub listen_retry_interval: Duration,
}

/// Transactional emails, such as email verification and password reset (IF-12). They go over SMTP
/// to an external email delivery service (DE-01), which delivers them from a TwoViewer domain
/// whose sender authentication (SPF, DKIM and DMARC) is set up with that service. The SMTP user
/// name and password are secrets ([`SmtpCredentials`]).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Email {
    /// The address emails come from, on the TwoViewer domain. There is no default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub from_address: Option<String>,
    /// The SMTP server of the email delivery service. There is no default.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub smtp_host: Option<String>,
    pub smtp_port: u16,
    pub smtp_security: SmtpSecurity,
    /// How long the SMTP server gets to answer each command, and to accept a connection.
    #[serde(with = "humantime_serde")]
    pub timeout: Duration,
    /// How often an email is tried before it is given up, including the first attempt. Only
    /// temporary failures are tried again, such as a refused connection or a 4xx reply; a 5xx
    /// reply is final.
    pub max_attempts: u32,
    /// How long to wait before trying an email again. The wait doubles after each further
    /// failure.
    #[serde(with = "humantime_serde")]
    pub retry_delay: Duration,
    /// How many emails may wait to be sent, including those waiting to be tried again. Further
    /// emails are dropped until there is room.
    pub max_pending: usize,
    /// How many connections to the SMTP server may be open at once.
    pub max_connections: u32,
}

/// How the connection to the SMTP server is encrypted.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum SmtpSecurity {
    /// Encrypted after connecting, with STARTTLS, which the server must offer. Usually port 587.
    Starttls,
    /// Encrypted from the start. Usually port 465.
    Tls,
    /// Not encrypted: for a local mail catcher in development only.
    None,
}

/// Operational logs (MNT-03), written to standard output.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Log {
    pub format: LogFormat,
    /// Which logs to write, as `tracing` filter directives, such as `info` or
    /// `info,twoviewer_server=debug`.
    pub filter: String,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogFormat {
    /// One JSON object per line, for log collectors.
    Json,
    /// Human-readable lines, for development.
    Text,
}

impl Default for Http {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([0, 0, 0, 0], 8443)),
            redirect_listen: SocketAddr::from(([0, 0, 0, 0], 8080)),
            public_port: 443,
            hsts_max_age: Duration::from_secs(365 * 24 * 60 * 60),
            shutdown_timeout: Duration::from_secs(30),
        }
    }
}

impl Default for Tls {
    fn default() -> Self {
        Self {
            certificate_file: None,
            private_key_file: None,
            self_signed: false,
            reload_interval: Duration::from_secs(5 * 60),
            handshake_timeout: Duration::from_secs(10),
        }
    }
}

impl Default for Monitoring {
    fn default() -> Self {
        Self {
            listen: SocketAddr::from(([0, 0, 0, 0], 9090)),
            readiness_timeout: Duration::from_secs(2),
        }
    }
}

impl Default for Database {
    fn default() -> Self {
        Self {
            migration_lock_timeout: Duration::from_secs(10),
        }
    }
}

impl Default for Realtime {
    fn default() -> Self {
        Self {
            heartbeat_interval: Duration::from_secs(3),
            heartbeat_timeout: Duration::from_secs(8),
            max_message_bytes: 64 * 1024,
            send_queue_length: 64,
            listen_retry_interval: Duration::from_secs(1),
        }
    }
}

impl Default for Email {
    fn default() -> Self {
        Self {
            from_address: None,
            smtp_host: None,
            smtp_port: 587,
            smtp_security: SmtpSecurity::Starttls,
            timeout: Duration::from_secs(30),
            max_attempts: 5,
            retry_delay: Duration::from_secs(60),
            max_pending: 1000,
            max_connections: 10,
        }
    }
}

impl Default for Log {
    fn default() -> Self {
        Self {
            format: LogFormat::Json,
            filter: "info".to_owned(),
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum ConfigError {
    #[error("invalid configuration: {0}")]
    Invalid(#[from] Box<figment::Error>),
    #[error("invalid configuration: {key} must be at least 1ms")]
    TooShort { key: &'static str },
    #[error("invalid configuration: {key} must be at least 1")]
    TooSmall { key: &'static str },
    #[error(
        "invalid configuration: realtime.heartbeat_timeout must be longer than \
         realtime.heartbeat_interval"
    )]
    HeartbeatTimeout,
    #[error(
        "invalid configuration: set either tls.certificate_file and tls.private_key_file, or \
         tls.self_signed for development"
    )]
    CertificateSource,
    #[error(
        "invalid configuration: set email.from_address and email.smtp_host, the sender address \
         and the SMTP server of the email delivery service"
    )]
    EmailServer,
    #[error("invalid configuration: email.from_address is not an email address")]
    FromAddress,
    #[error(
        "invalid configuration: with email.smtp_security = \"none\", SMTP_USERNAME and \
         SMTP_PASSWORD would be sent unencrypted"
    )]
    CredentialsInClear,
    #[error("{0} is not set")]
    MissingSecret(&'static str),
}

impl Config {
    /// Reads the configuration from the defaults, the file and the environment.
    pub fn load() -> Result<Self, ConfigError> {
        let mut figment = Figment::from(Serialized::defaults(Config::default()));
        if let Some(path) = std::env::var_os(FILE_VAR) {
            // Unlike `Toml::file`, a missing file is an error rather than silently skipped.
            figment = figment.merge(Toml::file_exact(path));
        }
        let config: Config = figment
            .merge(Env::prefixed(ENV_PREFIX).ignore(&["config"]).split("__"))
            .extract()
            .map_err(Box::new)?;
        config.validate()?;
        Ok(config)
    }

    fn validate(&self) -> Result<(), ConfigError> {
        // Durations reach PostgreSQL in whole milliseconds, where 0 means no timeout at all. A
        // zero reload interval or handshake timeout would make no sense either.
        for (key, value) in [
            ("http.shutdown_timeout", self.http.shutdown_timeout),
            ("tls.reload_interval", self.tls.reload_interval),
            ("tls.handshake_timeout", self.tls.handshake_timeout),
            (
                "monitoring.readiness_timeout",
                self.monitoring.readiness_timeout,
            ),
            (
                "database.migration_lock_timeout",
                self.database.migration_lock_timeout,
            ),
            (
                "realtime.heartbeat_interval",
                self.realtime.heartbeat_interval,
            ),
            (
                "realtime.listen_retry_interval",
                self.realtime.listen_retry_interval,
            ),
            ("email.timeout", self.email.timeout),
            ("email.retry_delay", self.email.retry_delay),
        ] {
            if value < Duration::from_millis(1) {
                return Err(ConfigError::TooShort { key });
            }
        }
        for (key, value) in [
            (
                "realtime.max_message_bytes",
                self.realtime.max_message_bytes,
            ),
            (
                "realtime.send_queue_length",
                self.realtime.send_queue_length,
            ),
            ("email.max_attempts", self.email.max_attempts as usize),
            ("email.max_pending", self.email.max_pending),
            ("email.max_connections", self.email.max_connections as usize),
        ] {
            if value == 0 {
                return Err(ConfigError::TooSmall { key });
            }
        }
        if self.realtime.heartbeat_timeout <= self.realtime.heartbeat_interval {
            return Err(ConfigError::HeartbeatTimeout);
        }
        let tls = &self.tls;
        match (
            &tls.certificate_file,
            &tls.private_key_file,
            tls.self_signed,
        ) {
            (Some(_), Some(_), false) | (None, None, true) => {}
            _ => return Err(ConfigError::CertificateSource),
        }
        self.email.server()?;
        Ok(())
    }
}

impl Email {
    /// The sender address and the SMTP server, which have no defaults.
    pub fn server(&self) -> Result<(Address, &str), ConfigError> {
        let (Some(from), Some(host)) = (&self.from_address, &self.smtp_host) else {
            return Err(ConfigError::EmailServer);
        };
        let from = from.parse().map_err(|_| ConfigError::FromAddress)?;
        Ok((from, host))
    }
}

/// Secrets, provided at runtime and never stored in source control (SEC-13).
#[derive(Clone, Debug)]
pub struct Secrets {
    pub database_url: DatabaseUrl,
    /// For SMTP servers that require authentication, which those of email delivery services do.
    pub smtp_credentials: Option<SmtpCredentials>,
}

impl Secrets {
    /// Reads the secrets from the environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        let database_url =
            secret("DATABASE_URL").ok_or(ConfigError::MissingSecret("DATABASE_URL"))?;
        Ok(Self {
            database_url: DatabaseUrl::new(database_url),
            smtp_credentials: SmtpCredentials::from_env()?,
        })
    }
}

/// The value of the environment variable `name`, unless it is unset or empty.
fn secret(name: &str) -> Option<String> {
    std::env::var(name).ok().filter(|value| !value.is_empty())
}

/// A PostgreSQL connection URL. It contains the database password, so it never appears in logs
/// (MNT-03): `Debug` hides it, and there is no `Display`.
#[derive(Clone)]
pub struct DatabaseUrl(String);

impl DatabaseUrl {
    pub fn new(url: impl Into<String>) -> Self {
        Self(url.into())
    }

    /// The URL itself, to connect with. Never log it.
    pub fn expose(&self) -> &str {
        &self.0
    }
}

impl fmt::Debug for DatabaseUrl {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("DatabaseUrl(..)")
    }
}

/// The user name and password for the SMTP server. They never appear in logs (MNT-03): `Debug`
/// hides them, and there is no `Display`.
#[derive(Clone)]
pub struct SmtpCredentials {
    username: String,
    password: String,
}

impl SmtpCredentials {
    pub fn new(username: impl Into<String>, password: impl Into<String>) -> Self {
        Self {
            username: username.into(),
            password: password.into(),
        }
    }

    /// Reads `SMTP_USERNAME` and `SMTP_PASSWORD` from the environment: both, or neither for an
    /// SMTP server without authentication.
    pub fn from_env() -> Result<Option<Self>, ConfigError> {
        match (secret("SMTP_USERNAME"), secret("SMTP_PASSWORD")) {
            (Some(username), Some(password)) => Ok(Some(Self::new(username, password))),
            (None, None) => Ok(None),
            (Some(_), None) => Err(ConfigError::MissingSecret("SMTP_PASSWORD")),
            (None, Some(_)) => Err(ConfigError::MissingSecret("SMTP_USERNAME")),
        }
    }

    pub fn username(&self) -> &str {
        &self.username
    }

    /// The password itself, to authenticate with. Never log it.
    pub fn expose_password(&self) -> &str {
        &self.password
    }
}

impl fmt::Debug for SmtpCredentials {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("SmtpCredentials(..)")
    }
}

#[cfg(test)]
mod tests {
    use figment::Jail;

    use super::*;

    const EXAMPLE: &str = include_str!("../config.example.toml");

    /// Runs `test` with a clean environment, in a temporary working directory.
    #[expect(
        clippy::result_large_err,
        reason = "the closure's error type is fixed by `Jail::expect_with`"
    )]
    fn jail(test: impl FnOnce(&mut Jail)) {
        Jail::expect_with(|jail| {
            jail.clear_env();
            test(jail);
            Ok(())
        });
    }

    /// Sets the settings without a default: a certificate and the email server.
    fn set_required(jail: &mut Jail) {
        jail.set_env("TWOVIEWER_TLS__SELF_SIGNED", "true");
        set_email_server(jail);
    }

    fn set_email_server(jail: &mut Jail) {
        jail.set_env("TWOVIEWER_EMAIL__FROM_ADDRESS", "no-reply@twoviewer.test");
        jail.set_env("TWOVIEWER_EMAIL__SMTP_HOST", "smtp.example.com");
    }

    #[test]
    fn example_file_lists_every_setting_with_its_default() {
        let example: Config = Figment::from(Toml::string(EXAMPLE)).extract().unwrap();

        assert_eq!(example, Config::default());
        // Settings without a default are shown commented out.
        for key in [
            "certificate_file",
            "private_key_file",
            "from_address",
            "smtp_host",
        ] {
            assert!(EXAMPLE.contains(&format!("# {key} = ")), "{key}");
        }
    }

    #[test]
    fn defaults_need_a_certificate() {
        jail(|_| {
            assert!(matches!(
                Config::load(),
                Err(ConfigError::CertificateSource)
            ));
        });
    }

    #[test]
    fn defaults_apply_without_file_or_environment() {
        jail(|jail| {
            set_required(jail);
            let mut expected = Config::default();
            expected.tls.self_signed = true;
            expected.email.from_address = Some("no-reply@twoviewer.test".to_owned());
            expected.email.smtp_host = Some("smtp.example.com".to_owned());

            assert_eq!(Config::load().unwrap(), expected);
        });
    }

    #[test]
    fn exactly_one_certificate_source() {
        for (certificate, key, self_signed, valid) in [
            (true, true, false, true),
            (true, true, true, false),
            (true, false, false, false),
            (false, true, false, false),
            (true, false, true, false),
        ] {
            let mut config = Config::default();
            config.email.from_address = Some("no-reply@twoviewer.test".to_owned());
            config.email.smtp_host = Some("smtp.example.com".to_owned());
            config.tls.certificate_file = certificate.then(|| "tls.crt".into());
            config.tls.private_key_file = key.then(|| "tls.key".into());
            config.tls.self_signed = self_signed;

            assert_eq!(
                config.validate().is_ok(),
                valid,
                "certificate {certificate}, key {key}, self-signed {self_signed}"
            );
        }
    }

    #[test]
    fn certificate_files_can_be_set() {
        jail(|jail| {
            jail.set_env("TWOVIEWER_TLS__CERTIFICATE_FILE", "/run/tls/tls.crt");
            jail.set_env("TWOVIEWER_TLS__PRIVATE_KEY_FILE", "/run/tls/tls.key");
            set_email_server(jail);

            let tls = Config::load().unwrap().tls;

            assert_eq!(tls.certificate_file, Some("/run/tls/tls.crt".into()));
            assert_eq!(tls.private_key_file, Some("/run/tls/tls.key".into()));
        });
    }

    #[test]
    fn environment_overrides_file_overrides_defaults() {
        jail(|jail| {
            jail.create_file(
                "config.toml",
                r#"
                    [http]
                    listen = "127.0.0.1:8181"
                    [database]
                    migration_lock_timeout = "1m"
                "#,
            )
            .unwrap();
            jail.set_env(FILE_VAR, "config.toml");
            jail.set_env("TWOVIEWER_DATABASE__MIGRATION_LOCK_TIMEOUT", "250ms");
            jail.set_env("TWOVIEWER_LOG__FORMAT", "text");
            set_required(jail);

            let config = Config::load().unwrap();

            assert_eq!(config.http.listen, "127.0.0.1:8181".parse().unwrap());
            assert_eq!(
                config.http.shutdown_timeout,
                Http::default().shutdown_timeout
            );
            assert_eq!(
                config.database.migration_lock_timeout,
                Duration::from_millis(250)
            );
            assert_eq!(config.log.format, LogFormat::Text);
        });
    }

    #[test]
    fn unknown_settings_are_rejected() {
        jail(|jail| {
            jail.set_env("TWOVIEWER_DATABASE__MIGRATION_LOCK_TIMOUT", "1s");

            let err = Config::load().unwrap_err().to_string();

            assert!(err.contains("migration_lock_timout"), "{err}");
        });
    }

    #[test]
    fn secrets_cannot_be_set_in_the_file() {
        jail(|jail| {
            jail.create_file(
                "config.toml",
                "[database]\nurl = \"postgres://app:hunter2@db/app\"",
            )
            .unwrap();
            jail.set_env(FILE_VAR, "config.toml");

            let err = Config::load().unwrap_err().to_string();

            assert!(err.contains("unknown field"), "{err}");
            assert!(!err.contains("hunter2"), "{err}");
        });
    }

    #[test]
    fn a_missing_file_is_an_error() {
        jail(|jail| {
            jail.set_env(FILE_VAR, "missing.toml");

            assert!(Config::load().is_err());
        });
    }

    #[test]
    fn invalid_values_name_the_setting() {
        jail(|jail| {
            jail.set_env("TWOVIEWER_MONITORING__READINESS_TIMEOUT", "soon");

            let err = Config::load().unwrap_err().to_string();

            assert!(
                err.to_lowercase().contains("monitoring.readiness_timeout"),
                "{err}"
            );
        });
    }

    #[test]
    fn timeouts_below_a_millisecond_are_rejected() {
        jail(|jail| {
            jail.set_env("TWOVIEWER_DATABASE__MIGRATION_LOCK_TIMEOUT", "0s");

            assert!(matches!(
                Config::load(),
                Err(ConfigError::TooShort {
                    key: "database.migration_lock_timeout"
                })
            ));
        });
    }

    #[test]
    fn heartbeat_timeout_must_exceed_the_interval() {
        jail(|jail| {
            jail.set_env("TWOVIEWER_TLS__SELF_SIGNED", "true");
            jail.set_env("TWOVIEWER_REALTIME__HEARTBEAT_INTERVAL", "8s");

            assert!(matches!(Config::load(), Err(ConfigError::HeartbeatTimeout)));
        });
    }

    #[test]
    fn realtime_counts_must_be_positive() {
        jail(|jail| {
            jail.set_env("TWOVIEWER_TLS__SELF_SIGNED", "true");
            jail.set_env("TWOVIEWER_REALTIME__SEND_QUEUE_LENGTH", "0");

            assert!(matches!(
                Config::load(),
                Err(ConfigError::TooSmall {
                    key: "realtime.send_queue_length"
                })
            ));
        });
    }

    #[test]
    fn hsts_max_age_may_be_zero() {
        jail(|jail| {
            set_required(jail);
            jail.set_env("TWOVIEWER_HTTP__HSTS_MAX_AGE", "0s");

            assert_eq!(Config::load().unwrap().http.hsts_max_age, Duration::ZERO);
        });
    }

    #[test]
    fn secrets_come_from_the_environment() {
        jail(|jail| {
            assert!(matches!(
                Secrets::from_env(),
                Err(ConfigError::MissingSecret("DATABASE_URL"))
            ));

            jail.set_env("DATABASE_URL", "postgres://app:hunter2@db/app");
            let secrets = Secrets::from_env().unwrap();

            assert_eq!(
                secrets.database_url.expose(),
                "postgres://app:hunter2@db/app"
            );
        });
    }

    #[test]
    fn secrets_are_not_formatted() {
        let secrets = Secrets {
            database_url: DatabaseUrl::new("postgres://app:hunter2@db/app"),
            smtp_credentials: Some(SmtpCredentials::new("apikey-7", "hunter3")),
        };

        let formatted = format!("{secrets:?}");

        for secret in ["hunter2", "apikey-7", "hunter3"] {
            assert!(!formatted.contains(secret), "{formatted}");
        }
    }

    #[test]
    fn email_needs_a_sender_and_a_server() {
        jail(|jail| {
            jail.set_env("TWOVIEWER_TLS__SELF_SIGNED", "true");
            assert!(matches!(Config::load(), Err(ConfigError::EmailServer)));

            jail.set_env("TWOVIEWER_EMAIL__FROM_ADDRESS", "no-reply@twoviewer.test");
            assert!(matches!(Config::load(), Err(ConfigError::EmailServer)));

            jail.set_env("TWOVIEWER_EMAIL__SMTP_HOST", "smtp.example.com");
            let config = Config::load().unwrap();
            let (from, host) = config.email.server().unwrap();
            assert_eq!(from.to_string(), "no-reply@twoviewer.test");
            assert_eq!(host, "smtp.example.com");
        });
    }

    #[test]
    fn the_sender_must_be_an_email_address() {
        jail(|jail| {
            set_required(jail);
            jail.set_env("TWOVIEWER_EMAIL__FROM_ADDRESS", "TwoViewer");

            assert!(matches!(Config::load(), Err(ConfigError::FromAddress)));
        });
    }

    #[test]
    fn smtp_security_is_one_of_three() {
        jail(|jail| {
            set_required(jail);
            jail.set_env("TWOVIEWER_EMAIL__SMTP_SECURITY", "tls");
            assert_eq!(
                Config::load().unwrap().email.smtp_security,
                SmtpSecurity::Tls
            );

            jail.set_env("TWOVIEWER_EMAIL__SMTP_SECURITY", "ssl");
            assert!(Config::load().is_err());
        });
    }

    #[test]
    fn email_counts_must_be_positive() {
        jail(|jail| {
            set_required(jail);
            jail.set_env("TWOVIEWER_EMAIL__MAX_ATTEMPTS", "0");

            assert!(matches!(
                Config::load(),
                Err(ConfigError::TooSmall {
                    key: "email.max_attempts"
                })
            ));
        });
    }

    #[test]
    fn smtp_credentials_come_in_pairs() {
        jail(|jail| {
            assert!(SmtpCredentials::from_env().unwrap().is_none());

            jail.set_env("SMTP_USERNAME", "apikey");
            assert!(matches!(
                SmtpCredentials::from_env(),
                Err(ConfigError::MissingSecret("SMTP_PASSWORD"))
            ));

            jail.set_env("SMTP_PASSWORD", "hunter3");
            let credentials = SmtpCredentials::from_env().unwrap().unwrap();
            assert_eq!(credentials.username(), "apikey");
            assert_eq!(credentials.expose_password(), "hunter3");

            jail.set_env("SMTP_USERNAME", "");
            assert!(matches!(
                SmtpCredentials::from_env(),
                Err(ConfigError::MissingSecret("SMTP_USERNAME"))
            ));
        });
    }
}
