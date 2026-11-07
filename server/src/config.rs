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

use axum::http::Uri;
use figment::{
    Figment,
    providers::{Env, Format, Serialized, Toml},
};
use lettre::Address;
use serde::{Deserialize, Serialize};
use zeroize::Zeroizing;

use crate::accounts::password;

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
    pub passwords: Passwords,
    pub accounts: Accounts,
    pub sign_up: SignUp,
    pub sign_in: SignIn,
    pub mfa: Mfa,
    pub password_reset: PasswordReset,
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
    /// The address people reach the web client at, such as `https://twoviewer.example`, which
    /// links in emails point to. There is no default: links are never built from the `Host`
    /// header of a request, which the client chooses.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub public_url: Option<String>,
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

/// Passwords: the rules for new ones (AUTH-03) and how they are stored (SEC-07).
///
/// Lengths count characters, meaning Unicode code points after normalization, not bytes. Hashes
/// are Argon2id, which costs `hash_iterations` passes over `hash_memory_kib` of memory each time a
/// password is set or checked. Raising the cost affects new hashes at once, and the others at their
/// next sign-in.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Passwords {
    /// The fewest characters a new password may have.
    pub min_length: usize,
    /// The most characters a new password may have. AUTH-03 requires accepting at least 128.
    pub max_length: usize,
    /// The memory each hash takes, in KiB.
    pub hash_memory_kib: u32,
    /// How many passes each hash makes over its memory.
    pub hash_iterations: u32,
    /// How many lanes the memory is divided into. They are computed one after the other, so more
    /// lanes don't make a hash faster.
    pub hash_parallelism: u32,
    /// How many passwords may be hashed or checked at once. Further ones wait, so hashing takes at
    /// most this many times `hash_memory_kib` of memory, however many requests arrive.
    pub max_concurrent_hashes: usize,
}

/// User accounts (§3.1).
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Accounts {
    /// How long a link that verifies an email address works (AUTH-02).
    #[serde(with = "humantime_serde")]
    pub verification_link_lifetime: Duration,
    /// The most characters a display name may have.
    pub display_name_max_length: usize,
}

/// Protection of sign-up against automated abuse (AUTH-15), without third-party services.
///
/// Sign-ups are counted per network: one IPv4 address, or one IPv6 /64. Past `challenge_after`
/// within `window`, the browser must solve a proof-of-work challenge first; past
/// `max_per_network`, sign-up is refused until the window has moved on. The counts are kept in
/// memory by each instance of the server.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignUp {
    /// The time over which sign-ups from one network are counted.
    #[serde(with = "humantime_serde")]
    pub window: Duration,
    /// How many sign-ups one network may make within `window` without a challenge.
    pub challenge_after: usize,
    /// How many sign-ups one network may make within `window` at all.
    pub max_per_network: usize,
    /// How many networks are counted at once. When that many are, sign-ups from other networks
    /// need a challenge, so the counts take bounded memory.
    pub max_tracked_networks: usize,
    /// How hard a challenge is: the number of leading zero bits the browser must find in a hash.
    /// Each further bit doubles the work.
    pub challenge_difficulty: u32,
    /// How long the browser has to solve a challenge.
    #[serde(with = "humantime_serde")]
    pub challenge_lifetime: Duration,
    /// The least time between two emails that sign-ups send to one address, so that sign-up
    /// cannot be used to flood someone's inbox.
    #[serde(with = "humantime_serde")]
    pub notice_interval: Duration,
}

/// Sign-in (AUTH-05) and its protection against guessing (AUTH-08, SEC-08).
///
/// Failed sign-ins are counted per email address, whether or not the address has an account, so
/// that a block doesn't reveal whether it has one (SEC-14). These counts are in the database, so
/// they hold across instances and restarts. Failures are also counted per network, one IPv4 address
/// or one IPv6 /64, in memory by each instance, which slows down guessing across many addresses.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct SignIn {
    /// How many failed sign-ins with one email address within `failure_window` block sign-in with
    /// that address (AUTH-08).
    pub max_failures: usize,
    /// The time over which failed sign-ins are counted, per address and per network.
    #[serde(with = "humantime_serde")]
    pub failure_window: Duration,
    /// How long sign-in with an address stays blocked once it has failed `max_failures` times. The
    /// owner of the account, if there is one, is emailed (AUTH-08).
    #[serde(with = "humantime_serde")]
    pub block_duration: Duration,
    /// How many failed sign-ins one network may make within `failure_window`. Further attempts are
    /// refused until the window has moved on.
    pub max_failures_per_network: usize,
    /// How many networks are counted at once, so that counting takes bounded memory. While that
    /// many are, failures from other networks aren't counted.
    pub max_tracked_networks: usize,
    /// How long a sign-in lasts at most, however much it is used (AUTH-09). The sign-in cookie is
    /// kept that long.
    #[serde(with = "humantime_serde")]
    pub session_lifetime: Duration,
    /// How long a sign-in lasts without being used (AUTH-09).
    #[serde(with = "humantime_serde")]
    pub idle_timeout: Duration,
    /// How long after the password was last entered, at sign-in or since, sensitive changes need
    /// it again, with the second factor if MFA is on (AUTH-10).
    #[serde(with = "humantime_serde")]
    pub reauthenticate_after: Duration,
}

/// Multi-factor authentication with an authenticator app (AUTH-05, AUTH-06), and its protection
/// against guessing (SEC-08).
///
/// Wrong codes are counted per account, in the database like failed sign-ins, both at sign-in and
/// at re-authentication. Recovery codes count the same.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Mfa {
    /// How many wrong codes for one account within `failure_window` block its codes.
    pub max_failures: usize,
    /// The time over which wrong codes are counted.
    #[serde(with = "humantime_serde")]
    pub failure_window: Duration,
    /// How long codes for an account stay blocked once `max_failures` were wrong.
    #[serde(with = "humantime_serde")]
    pub block_duration: Duration,
    /// How long the second step of sign-in waits for a code once the password was right.
    #[serde(with = "humantime_serde")]
    pub sign_in_step_timeout: Duration,
    /// How many 30-second steps a code may be early or late, for clocks that are off.
    pub clock_skew_steps: u32,
}

/// Password reset by email (AUTH-07), and its protection against abuse (SEC-08).
///
/// Requests are counted per network, one IPv4 address or one IPv6 /64, and reset emails per
/// address, both in memory by each instance of the server, like sign-ups.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PasswordReset {
    /// How long a reset link works (AUTH-07).
    #[serde(with = "humantime_serde")]
    pub link_lifetime: Duration,
    /// The time over which reset requests from one network are counted.
    #[serde(with = "humantime_serde")]
    pub window: Duration,
    /// How many reset requests one network may make within `window`. Further ones are refused
    /// until the window has moved on.
    pub max_per_network: usize,
    /// How many networks are counted at once, so that counting takes bounded memory. While that
    /// many are, requests from other networks aren't counted: the limit per address still holds.
    pub max_tracked_networks: usize,
    /// The least time between two reset emails to one address, so that reset requests cannot be
    /// used to flood someone's inbox.
    #[serde(with = "humantime_serde")]
    pub email_interval: Duration,
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
            public_url: None,
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

impl Default for Passwords {
    fn default() -> Self {
        Self {
            min_length: 12,
            max_length: 1024,
            // OWASP's recommended minimum for Argon2id: 19 MiB, 2 passes, 1 lane.
            hash_memory_kib: 19 * 1024,
            hash_iterations: 2,
            hash_parallelism: 1,
            max_concurrent_hashes: 4,
        }
    }
}

impl Default for Accounts {
    fn default() -> Self {
        Self {
            verification_link_lifetime: Duration::from_secs(24 * 60 * 60),
            display_name_max_length: 100,
        }
    }
}

impl Default for SignUp {
    fn default() -> Self {
        Self {
            window: Duration::from_secs(60 * 60),
            challenge_after: 3,
            max_per_network: 30,
            max_tracked_networks: 100_000,
            challenge_difficulty: DEFAULT_CHALLENGE_DIFFICULTY,
            challenge_lifetime: Duration::from_secs(10 * 60),
            notice_interval: Duration::from_secs(15 * 60),
        }
    }
}

impl Default for SignIn {
    fn default() -> Self {
        Self {
            max_failures: 10,
            failure_window: Duration::from_secs(15 * 60),
            block_duration: Duration::from_secs(15 * 60),
            max_failures_per_network: 50,
            max_tracked_networks: 100_000,
            session_lifetime: Duration::from_secs(30 * 24 * 60 * 60),
            idle_timeout: Duration::from_secs(7 * 24 * 60 * 60),
            reauthenticate_after: Duration::from_secs(15 * 60),
        }
    }
}

impl Default for Mfa {
    fn default() -> Self {
        Self {
            max_failures: 5,
            failure_window: Duration::from_secs(15 * 60),
            block_duration: Duration::from_secs(15 * 60),
            sign_in_step_timeout: Duration::from_secs(5 * 60),
            clock_skew_steps: 1,
        }
    }
}

impl Default for PasswordReset {
    fn default() -> Self {
        Self {
            link_lifetime: Duration::from_secs(60 * 60),
            window: Duration::from_secs(60 * 60),
            max_per_network: 20,
            max_tracked_networks: 100_000,
            email_interval: Duration::from_secs(5 * 60),
        }
    }
}

/// About 131,000 hashes on average. The web client computes about 190,000 per second with
/// WebCrypto on a recent laptop, so this takes a second or two on most.
const DEFAULT_CHALLENGE_DIFFICULTY: u32 = 17;

/// The hardest challenge allowed. Each bit doubles the work, and 32 bits would take a browser
/// hours.
pub const MAX_CHALLENGE_DIFFICULTY: u32 = 32;

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
        "invalid configuration: set http.public_url, the address of the web client that links in \
         emails point to, such as https://twoviewer.example"
    )]
    PublicUrlMissing,
    #[error(
        "invalid configuration: http.public_url must be an https:// address without a path, such \
         as https://twoviewer.example; http:// is allowed only for localhost"
    )]
    PublicUrl,
    #[error("invalid configuration: passwords.max_length must be at least passwords.min_length")]
    PasswordLengths,
    #[error(
        "invalid configuration: sign_up.max_per_network must be at least sign_up.challenge_after"
    )]
    SignUpLimits,
    #[error(
        "invalid configuration: sign_up.challenge_difficulty must be at most {MAX_CHALLENGE_DIFFICULTY}"
    )]
    ChallengeDifficulty,
    #[error(
        "invalid configuration: passwords.hash_memory_kib, passwords.hash_iterations and \
         passwords.hash_parallelism are not valid Argon2 parameters: {0}"
    )]
    PasswordHashing(argon2::Error),
    #[error(
        "invalid configuration: with email.smtp_security = \"none\", SMTP_USERNAME and \
         SMTP_PASSWORD would be sent unencrypted"
    )]
    CredentialsInClear,
    #[error("{0} is not set")]
    MissingSecret(&'static str),
    #[error("{name} is not valid: it must be {expected}")]
    InvalidSecret {
        name: &'static str,
        expected: &'static str,
    },
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
            (
                "accounts.verification_link_lifetime",
                self.accounts.verification_link_lifetime,
            ),
            ("sign_up.window", self.sign_up.window),
            (
                "sign_up.challenge_lifetime",
                self.sign_up.challenge_lifetime,
            ),
            ("sign_up.notice_interval", self.sign_up.notice_interval),
            ("sign_in.failure_window", self.sign_in.failure_window),
            ("sign_in.block_duration", self.sign_in.block_duration),
            ("sign_in.session_lifetime", self.sign_in.session_lifetime),
            ("sign_in.idle_timeout", self.sign_in.idle_timeout),
            (
                "sign_in.reauthenticate_after",
                self.sign_in.reauthenticate_after,
            ),
            ("mfa.failure_window", self.mfa.failure_window),
            ("mfa.block_duration", self.mfa.block_duration),
            ("mfa.sign_in_step_timeout", self.mfa.sign_in_step_timeout),
            (
                "password_reset.link_lifetime",
                self.password_reset.link_lifetime,
            ),
            ("password_reset.window", self.password_reset.window),
            (
                "password_reset.email_interval",
                self.password_reset.email_interval,
            ),
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
            ("passwords.min_length", self.passwords.min_length),
            (
                "passwords.max_concurrent_hashes",
                self.passwords.max_concurrent_hashes,
            ),
            (
                "accounts.display_name_max_length",
                self.accounts.display_name_max_length,
            ),
            ("sign_up.max_per_network", self.sign_up.max_per_network),
            (
                "sign_up.max_tracked_networks",
                self.sign_up.max_tracked_networks,
            ),
            ("sign_in.max_failures", self.sign_in.max_failures),
            (
                "sign_in.max_failures_per_network",
                self.sign_in.max_failures_per_network,
            ),
            (
                "sign_in.max_tracked_networks",
                self.sign_in.max_tracked_networks,
            ),
            ("mfa.max_failures", self.mfa.max_failures),
            (
                "password_reset.max_per_network",
                self.password_reset.max_per_network,
            ),
            (
                "password_reset.max_tracked_networks",
                self.password_reset.max_tracked_networks,
            ),
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
        self.http.public_url()?;
        if self.passwords.max_length < self.passwords.min_length {
            return Err(ConfigError::PasswordLengths);
        }
        password::argon2_params(&self.passwords).map_err(ConfigError::PasswordHashing)?;
        if self.sign_up.max_per_network < self.sign_up.challenge_after {
            return Err(ConfigError::SignUpLimits);
        }
        if self.sign_up.challenge_difficulty > MAX_CHALLENGE_DIFFICULTY {
            return Err(ConfigError::ChallengeDifficulty);
        }
        Ok(())
    }
}

impl Http {
    /// The address of the web client, which has no default.
    pub fn public_url(&self) -> Result<PublicUrl, ConfigError> {
        PublicUrl::parse(
            self.public_url
                .as_deref()
                .ok_or(ConfigError::PublicUrlMissing)?,
        )
    }
}

/// The address people reach the web client at: an origin such as `https://twoviewer.example`,
/// without a path. Links in emails are made from it, never from the `Host` header of a request,
/// which would let anyone who sends a request choose where the link points.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct PublicUrl(String);

impl PublicUrl {
    /// Accepts `https` addresses, and `http` only for `localhost`, as for a development server.
    pub fn parse(url: &str) -> Result<Self, ConfigError> {
        let uri: Uri = url.parse().map_err(|_| ConfigError::PublicUrl)?;
        let (Some(scheme), Some(authority)) = (uri.scheme_str(), uri.authority()) else {
            return Err(ConfigError::PublicUrl);
        };
        let scheme = scheme.to_ascii_lowercase();
        let host = authority.host().to_ascii_lowercase();
        let local = matches!(host.as_str(), "localhost" | "127.0.0.1" | "[::1]");
        // `Uri` drops a fragment without a word, so look for one in the text.
        let origin_only = matches!(uri.path(), "" | "/")
            && uri.query().is_none()
            && !url.contains('#')
            && !authority.as_str().contains('@');
        let secure = scheme == "https" || (scheme == "http" && local);
        if !origin_only || !secure {
            return Err(ConfigError::PublicUrl);
        }
        let port = authority
            .port()
            .map(|port| format!(":{port}"))
            .unwrap_or_default();
        Ok(Self(format!("{scheme}://{host}{port}")))
    }

    /// The address of `path` in the web client. `path` starts with `/`.
    pub fn join(&self, path: &str) -> String {
        debug_assert!(path.starts_with('/'), "{path}");
        format!("{}{path}", self.0)
    }

    pub fn as_str(&self) -> &str {
        &self.0
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
    /// Encrypts the secrets of users' authenticator apps (SEC-07).
    pub mfa_encryption_key: MfaEncryptionKey,
}

impl Secrets {
    /// Reads the secrets from the environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        let database_url =
            secret("DATABASE_URL").ok_or(ConfigError::MissingSecret("DATABASE_URL"))?;
        let mfa_encryption_key = secret(MfaEncryptionKey::VAR)
            .ok_or(ConfigError::MissingSecret(MfaEncryptionKey::VAR))
            .and_then(|value| MfaEncryptionKey::from_base64(&value))?;
        Ok(Self {
            database_url: DatabaseUrl::new(database_url),
            smtp_credentials: SmtpCredentials::from_env()?,
            mfa_encryption_key,
        })
    }
}

/// The AES-256 key that encrypts the secrets of authenticator apps at rest (SEC-07), from the
/// environment variable `MFA_ENCRYPTION_KEY`: 32 random bytes, base64-encoded, such as the output
/// of `openssl rand -base64 32`.
///
/// Losing it makes every user's authenticator app useless, so that they can only sign in with a
/// recovery code. It never appears in logs (MNT-03): `Debug` hides it, there is no `Display`, and
/// its memory is cleared when it is dropped.
#[derive(Clone)]
pub struct MfaEncryptionKey(Zeroizing<[u8; MfaEncryptionKey::LENGTH]>);

impl MfaEncryptionKey {
    /// The environment variable that holds the key.
    pub const VAR: &str = "MFA_ENCRYPTION_KEY";
    /// The length of the key in bytes.
    pub const LENGTH: usize = 32;

    pub fn new(bytes: [u8; Self::LENGTH]) -> Self {
        Self(Zeroizing::new(bytes))
    }

    /// The key from its base64 encoding, which must be of exactly [`Self::LENGTH`] bytes.
    pub fn from_base64(encoded: &str) -> Result<Self, ConfigError> {
        let decoded = data_encoding::BASE64
            .decode(encoded.trim().as_bytes())
            .map(Zeroizing::new)
            .ok();
        let bytes: [u8; Self::LENGTH] = decoded
            .and_then(|decoded| decoded.as_slice().try_into().ok())
            .ok_or(ConfigError::InvalidSecret {
                name: Self::VAR,
                expected: "32 bytes, base64-encoded, such as from `openssl rand -base64 32`",
            })?;
        Ok(Self::new(bytes))
    }

    /// The key itself, to encrypt and decrypt with. Never log it.
    pub fn expose(&self) -> &[u8; Self::LENGTH] {
        &self.0
    }
}

impl fmt::Debug for MfaEncryptionKey {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str("MfaEncryptionKey(..)")
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

    /// Sets the settings without a default: a certificate, the email server and the address of
    /// the web client.
    fn set_required(jail: &mut Jail) {
        jail.set_env("TWOVIEWER_TLS__SELF_SIGNED", "true");
        set_email_server(jail);
    }

    fn set_email_server(jail: &mut Jail) {
        jail.set_env("TWOVIEWER_EMAIL__FROM_ADDRESS", "no-reply@twoviewer.test");
        jail.set_env("TWOVIEWER_EMAIL__SMTP_HOST", "smtp.example.com");
        jail.set_env("TWOVIEWER_HTTP__PUBLIC_URL", "https://twoviewer.example");
    }

    /// A configuration with every setting that has no default, as `set_required` sets them.
    fn complete() -> Config {
        let mut config = Config::default();
        config.tls.self_signed = true;
        config.email.from_address = Some("no-reply@twoviewer.test".to_owned());
        config.email.smtp_host = Some("smtp.example.com".to_owned());
        config.http.public_url = Some("https://twoviewer.example".to_owned());
        config
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
            "public_url",
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

            assert_eq!(Config::load().unwrap(), complete());
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
            let mut config = complete();
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

    /// 32 bytes, base64-encoded: `0x00`, `0x01`, ..., `0x1f`.
    const MFA_KEY: &str = "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8=";

    #[test]
    fn secrets_come_from_the_environment() {
        jail(|jail| {
            assert!(matches!(
                Secrets::from_env(),
                Err(ConfigError::MissingSecret("DATABASE_URL"))
            ));

            jail.set_env("DATABASE_URL", "postgres://app:hunter2@db/app");
            assert!(matches!(
                Secrets::from_env(),
                Err(ConfigError::MissingSecret("MFA_ENCRYPTION_KEY"))
            ));

            jail.set_env("MFA_ENCRYPTION_KEY", MFA_KEY);
            let secrets = Secrets::from_env().unwrap();

            assert_eq!(
                secrets.database_url.expose(),
                "postgres://app:hunter2@db/app"
            );
            let expected: Vec<u8> = (0..32).collect();
            assert_eq!(&secrets.mfa_encryption_key.expose()[..], &expected[..]);
        });
    }

    #[test]
    fn the_mfa_key_must_be_32_bytes_of_base64() {
        assert!(MfaEncryptionKey::from_base64(MFA_KEY).is_ok());
        assert!(MfaEncryptionKey::from_base64(&format!(" {MFA_KEY}\n")).is_ok());

        for invalid in [
            "",
            "not base64!",
            // 31 and 33 bytes.
            "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHg==",
            "AAECAwQFBgcICQoLDA0ODxAREhMUFRYXGBkaGxwdHh8g",
        ] {
            let err = MfaEncryptionKey::from_base64(invalid).unwrap_err();
            assert!(
                matches!(
                    err,
                    ConfigError::InvalidSecret {
                        name: "MFA_ENCRYPTION_KEY",
                        ..
                    }
                ),
                "{invalid}"
            );
            assert!(!err.to_string().contains(invalid) || invalid.is_empty());
        }
    }

    #[test]
    fn secrets_are_not_formatted() {
        let secrets = Secrets {
            database_url: DatabaseUrl::new("postgres://app:hunter2@db/app"),
            smtp_credentials: Some(SmtpCredentials::new("apikey-7", "hunter3")),
            mfa_encryption_key: MfaEncryptionKey::from_base64(MFA_KEY).unwrap(),
        };

        let formatted = format!("{secrets:?}");

        for secret in ["hunter2", "apikey-7", "hunter3", "[0, 1, 2", "AAECAwQF"] {
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
            jail.set_env("TWOVIEWER_HTTP__PUBLIC_URL", "https://twoviewer.example");
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
    fn sign_in_limits_must_be_positive() {
        for (variable, key) in [
            ("TWOVIEWER_SIGN_IN__MAX_FAILURES", "sign_in.max_failures"),
            (
                "TWOVIEWER_SIGN_IN__MAX_FAILURES_PER_NETWORK",
                "sign_in.max_failures_per_network",
            ),
        ] {
            jail(|jail| {
                set_required(jail);
                jail.set_env(variable, "0");

                assert!(
                    matches!(Config::load(), Err(ConfigError::TooSmall { key: k }) if k == key),
                    "{variable}"
                );
            });
        }
        jail(|jail| {
            set_required(jail);
            jail.set_env("TWOVIEWER_SIGN_IN__BLOCK_DURATION", "0s");

            assert!(matches!(
                Config::load(),
                Err(ConfigError::TooShort {
                    key: "sign_in.block_duration"
                })
            ));
        });
    }

    #[test]
    fn mfa_limits_and_timeouts_must_be_positive() {
        jail(|jail| {
            set_required(jail);
            jail.set_env("TWOVIEWER_MFA__MAX_FAILURES", "0");

            assert!(matches!(
                Config::load(),
                Err(ConfigError::TooSmall {
                    key: "mfa.max_failures"
                })
            ));
        });
        for (variable, key) in [
            (
                "TWOVIEWER_SIGN_IN__REAUTHENTICATE_AFTER",
                "sign_in.reauthenticate_after",
            ),
            (
                "TWOVIEWER_MFA__SIGN_IN_STEP_TIMEOUT",
                "mfa.sign_in_step_timeout",
            ),
            ("TWOVIEWER_MFA__BLOCK_DURATION", "mfa.block_duration"),
        ] {
            jail(|jail| {
                set_required(jail);
                jail.set_env(variable, "0s");

                assert!(
                    matches!(Config::load(), Err(ConfigError::TooShort { key: k }) if k == key),
                    "{variable}"
                );
            });
        }
        // Clocks that are exactly right.
        jail(|jail| {
            set_required(jail);
            jail.set_env("TWOVIEWER_MFA__CLOCK_SKEW_STEPS", "0");

            assert_eq!(Config::load().unwrap().mfa.clock_skew_steps, 0);
        });
    }

    #[test]
    fn password_reset_limits_and_timeouts_must_be_positive() {
        for (variable, key) in [
            (
                "TWOVIEWER_PASSWORD_RESET__MAX_PER_NETWORK",
                "password_reset.max_per_network",
            ),
            (
                "TWOVIEWER_PASSWORD_RESET__MAX_TRACKED_NETWORKS",
                "password_reset.max_tracked_networks",
            ),
        ] {
            jail(|jail| {
                set_required(jail);
                jail.set_env(variable, "0");

                assert!(
                    matches!(Config::load(), Err(ConfigError::TooSmall { key: k }) if k == key),
                    "{variable}"
                );
            });
        }
        for (variable, key) in [
            (
                "TWOVIEWER_PASSWORD_RESET__LINK_LIFETIME",
                "password_reset.link_lifetime",
            ),
            ("TWOVIEWER_PASSWORD_RESET__WINDOW", "password_reset.window"),
            (
                "TWOVIEWER_PASSWORD_RESET__EMAIL_INTERVAL",
                "password_reset.email_interval",
            ),
        ] {
            jail(|jail| {
                set_required(jail);
                jail.set_env(variable, "0s");

                assert!(
                    matches!(Config::load(), Err(ConfigError::TooShort { key: k }) if k == key),
                    "{variable}"
                );
            });
        }
    }

    #[test]
    fn password_lengths_must_be_ordered() {
        jail(|jail| {
            set_required(jail);
            jail.set_env("TWOVIEWER_PASSWORDS__MAX_LENGTH", "11");
            assert!(matches!(Config::load(), Err(ConfigError::PasswordLengths)));

            jail.set_env("TWOVIEWER_PASSWORDS__MAX_LENGTH", "12");
            assert_eq!(Config::load().unwrap().passwords.max_length, 12);

            jail.set_env("TWOVIEWER_PASSWORDS__MIN_LENGTH", "0");
            assert!(matches!(
                Config::load(),
                Err(ConfigError::TooSmall {
                    key: "passwords.min_length"
                })
            ));
        });
    }

    #[test]
    fn password_hashing_needs_valid_argon2_parameters() {
        for (setting, value) in [
            ("HASH_PARALLELISM", "0"),
            ("HASH_ITERATIONS", "0"),
            // At least 8 KiB per lane.
            ("HASH_MEMORY_KIB", "7"),
        ] {
            jail(|jail| {
                set_required(jail);
                jail.set_env(format!("TWOVIEWER_PASSWORDS__{setting}"), value);

                let err = Config::load().unwrap_err();

                assert!(matches!(err, ConfigError::PasswordHashing(_)), "{setting}");
                assert!(err.to_string().contains("passwords.hash_"), "{err}");
            });
        }
    }

    #[test]
    fn password_hashing_needs_a_permit() {
        jail(|jail| {
            set_required(jail);
            jail.set_env("TWOVIEWER_PASSWORDS__MAX_CONCURRENT_HASHES", "0");

            assert!(matches!(
                Config::load(),
                Err(ConfigError::TooSmall {
                    key: "passwords.max_concurrent_hashes"
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

    #[test]
    fn the_web_client_needs_a_public_url() {
        jail(|jail| {
            set_required(jail);
            jail.set_env("TWOVIEWER_HTTP__PUBLIC_URL", "");
            assert!(matches!(Config::load(), Err(ConfigError::PublicUrl)));
        });
        let mut config = complete();
        config.http.public_url = None;
        assert!(matches!(
            config.validate(),
            Err(ConfigError::PublicUrlMissing)
        ));
    }

    #[test]
    fn the_public_url_is_an_https_origin() {
        for (url, origin) in [
            (
                "https://twoviewer.example",
                Some("https://twoviewer.example"),
            ),
            (
                "https://TwoViewer.Example/",
                Some("https://twoviewer.example"),
            ),
            (
                "https://twoviewer.example:8443",
                Some("https://twoviewer.example:8443"),
            ),
            ("http://localhost:4200", Some("http://localhost:4200")),
            ("http://127.0.0.1:4200", Some("http://127.0.0.1:4200")),
            ("http://[::1]:4200", Some("http://[::1]:4200")),
            // Plain HTTP would expose the links (SEC-01).
            ("http://twoviewer.example", None),
            ("ftp://twoviewer.example", None),
            ("twoviewer.example", None),
            ("/verify-email", None),
            ("https://twoviewer.example/app", None),
            ("https://twoviewer.example/?a=1", None),
            ("https://twoviewer.example/#top", None),
            ("https://admin@twoviewer.example", None),
        ] {
            assert_eq!(
                PublicUrl::parse(url).ok().as_ref().map(PublicUrl::as_str),
                origin,
                "{url}"
            );
        }
        assert_eq!(
            PublicUrl::parse("https://twoviewer.example/")
                .unwrap()
                .join("/verify-email"),
            "https://twoviewer.example/verify-email"
        );
    }

    #[test]
    fn sign_up_limits_must_be_consistent() {
        let mut config = complete();
        config.sign_up.challenge_after = 31;
        assert!(matches!(config.validate(), Err(ConfigError::SignUpLimits)));

        let mut config = complete();
        config.sign_up.challenge_after = 0;
        assert!(config.validate().is_ok());

        let mut config = complete();
        config.sign_up.challenge_difficulty = MAX_CHALLENGE_DIFFICULTY + 1;
        assert!(matches!(
            config.validate(),
            Err(ConfigError::ChallengeDifficulty)
        ));

        let mut config = complete();
        config.sign_up.max_tracked_networks = 0;
        assert!(matches!(
            config.validate(),
            Err(ConfigError::TooSmall {
                key: "sign_up.max_tracked_networks"
            })
        ));
    }
}
