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
//! `server/twoviewer.example.toml` lists every setting with its default. Unknown settings are
//! rejected, so a typo fails startup instead of being ignored. A new number goes into the section
//! of the feature that uses it, with a doc comment, and into the example file.
//!
//! Secrets are not settings. They come only from the environment at runtime ([`Secrets`]), never
//! from the file, which may well be kept in source control.

use std::{fmt, net::SocketAddr, path::PathBuf, time::Duration};

use figment::{
    Figment,
    providers::{Env, Format, Serialized, Toml},
};
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
    #[error(
        "invalid configuration: set either tls.certificate_file and tls.private_key_file, or \
         tls.self_signed for development"
    )]
    CertificateSource,
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
        ] {
            if value < Duration::from_millis(1) {
                return Err(ConfigError::TooShort { key });
            }
        }
        let tls = &self.tls;
        match (
            &tls.certificate_file,
            &tls.private_key_file,
            tls.self_signed,
        ) {
            (Some(_), Some(_), false) | (None, None, true) => Ok(()),
            _ => Err(ConfigError::CertificateSource),
        }
    }
}

/// Secrets, provided at runtime and never stored in source control (SEC-13).
#[derive(Clone, Debug)]
pub struct Secrets {
    pub database_url: DatabaseUrl,
}

impl Secrets {
    /// Reads the secrets from the environment.
    pub fn from_env() -> Result<Self, ConfigError> {
        let database_url = std::env::var("DATABASE_URL")
            .ok()
            .filter(|url| !url.is_empty())
            .ok_or(ConfigError::MissingSecret("DATABASE_URL"))?;
        Ok(Self {
            database_url: DatabaseUrl::new(database_url),
        })
    }
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

#[cfg(test)]
mod tests {
    use figment::Jail;

    use super::*;

    const EXAMPLE: &str = include_str!("../twoviewer.example.toml");

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

    #[test]
    fn example_file_lists_every_setting_with_its_default() {
        let example: Config = Figment::from(Toml::string(EXAMPLE)).extract().unwrap();

        assert_eq!(example, Config::default());
        // The certificate files have no default, so the example shows them commented out.
        for key in ["certificate_file", "private_key_file"] {
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
            jail.set_env("TWOVIEWER_TLS__SELF_SIGNED", "true");
            let mut expected = Config::default();
            expected.tls.self_signed = true;

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

            let tls = Config::load().unwrap().tls;

            assert_eq!(tls.certificate_file, Some("/run/tls/tls.crt".into()));
            assert_eq!(tls.private_key_file, Some("/run/tls/tls.key".into()));
        });
    }

    #[test]
    fn environment_overrides_file_overrides_defaults() {
        jail(|jail| {
            jail.create_file(
                "twoviewer.toml",
                r#"
                    [http]
                    listen = "127.0.0.1:8181"
                    [database]
                    migration_lock_timeout = "1m"
                "#,
            )
            .unwrap();
            jail.set_env(FILE_VAR, "twoviewer.toml");
            jail.set_env("TWOVIEWER_DATABASE__MIGRATION_LOCK_TIMEOUT", "250ms");
            jail.set_env("TWOVIEWER_LOG__FORMAT", "text");
            jail.set_env("TWOVIEWER_TLS__SELF_SIGNED", "true");

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
                "twoviewer.toml",
                "[database]\nurl = \"postgres://app:hunter2@db/app\"",
            )
            .unwrap();
            jail.set_env(FILE_VAR, "twoviewer.toml");

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
    fn hsts_max_age_may_be_zero() {
        jail(|jail| {
            jail.set_env("TWOVIEWER_TLS__SELF_SIGNED", "true");
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
    fn database_url_is_not_formatted() {
        let secrets = Secrets {
            database_url: DatabaseUrl::new("postgres://app:hunter2@db/app"),
        };

        assert!(!format!("{secrets:?}").contains("hunter2"));
    }
}
