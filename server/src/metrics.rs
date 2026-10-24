//! Operational metrics (MNT-04), served on the monitoring listener as OpenMetrics text.
//!
//! Features update the metrics through the handles on [`Metrics`]. Every metric is registered at
//! startup, so it reads 0 until its feature reports it, rather than missing. The exception is
//! `http_requests`, whose series appear with the first request for each route and status class.

use std::{fmt::Write as _, sync::atomic::AtomicU64};

use prometheus_client::{
    encoding::{EncodeLabelSet, EncodeLabelValue, LabelValueEncoder, text},
    metrics::{counter::Counter, family::Family, gauge::Gauge},
    registry::{Registry, Unit},
};

/// The content type of [`Metrics::encode`].
pub const CONTENT_TYPE: &str = "application/openmetrics-text; version=1.0.0; charset=utf-8";

pub struct Metrics {
    registry: Registry,
    /// Host Agents connected to the server right now.
    pub host_agents_online: Gauge,
    /// Sessions in progress, by type and transport.
    pub sessions_active: Family<SessionLabels, Gauge>,
    /// Session traffic forwarded by the relay. Its rate is the relay bandwidth.
    pub relay_bytes: Counter,
    /// Sign-in attempts that failed, for whatever reason.
    pub sign_ins_failed: Counter,
    /// Answered requests on the HTTPS listener, by route and status class. The 5xx share is the
    /// error rate.
    pub http_requests: Family<HttpLabels, Counter>,
    /// How long the migrations at startup took, including waiting for another instance (DB-07).
    pub migration_duration: Gauge<f64, AtomicU64>,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct SessionLabels {
    pub r#type: SessionType,
    pub transport: Transport,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum SessionType {
    Control,
    Vpn,
}

#[derive(Clone, Copy, Debug, Hash, PartialEq, Eq)]
pub enum Transport {
    Direct,
    Relayed,
}

#[derive(Clone, Debug, Hash, PartialEq, Eq, EncodeLabelSet)]
pub struct HttpLabels {
    /// The route template, such as `/devices/{id}`, or `unmatched`. Never the requested path,
    /// which would make one series per ID.
    pub route: String,
    /// `1xx` to `5xx`.
    pub status_class: &'static str,
}

impl Metrics {
    pub fn new() -> Self {
        let host_agents_online = Gauge::default();
        let sessions_active = Family::<SessionLabels, Gauge>::default();
        let relay_bytes = Counter::default();
        let sign_ins_failed = Counter::default();
        let http_requests = Family::default();
        let migration_duration = Gauge::default();

        let mut registry = Registry::with_prefix("twoviewer");
        registry.register(
            "host_agents_online",
            "Host Agents connected to the server",
            host_agents_online.clone(),
        );
        registry.register(
            "sessions_active",
            "Sessions in progress, by type and transport",
            sessions_active.clone(),
        );
        registry.register_with_unit(
            "relay",
            "Session traffic forwarded by the relay",
            Unit::Bytes,
            relay_bytes.clone(),
        );
        registry.register(
            "sign_ins_failed",
            "Failed sign-in attempts",
            sign_ins_failed.clone(),
        );
        registry.register(
            "http_requests",
            "Answered requests on the HTTPS listener, by route and status class",
            http_requests.clone(),
        );
        registry.register_with_unit(
            "migration_duration",
            "Duration of the migrations at startup",
            Unit::Seconds,
            migration_duration.clone(),
        );
        // Every combination reads 0 from the start, so dashboards show no sessions rather than
        // no data.
        for r#type in [SessionType::Control, SessionType::Vpn] {
            for transport in [Transport::Direct, Transport::Relayed] {
                let _ = sessions_active.get_or_create(&SessionLabels { r#type, transport });
            }
        }

        Self {
            registry,
            host_agents_online,
            sessions_active,
            relay_bytes,
            sign_ins_failed,
            http_requests,
            migration_duration,
        }
    }

    /// The current values in OpenMetrics text format ([`CONTENT_TYPE`]).
    pub fn encode(&self) -> String {
        let mut out = String::new();
        text::encode(&mut out, &self.registry).expect("writing to a String cannot fail");
        out
    }
}

impl Default for Metrics {
    fn default() -> Self {
        Self::new()
    }
}

/// The status class label of `status`, such as `4xx`.
pub fn status_class(status: u16) -> &'static str {
    match status {
        100..200 => "1xx",
        200..300 => "2xx",
        300..400 => "3xx",
        400..500 => "4xx",
        _ => "5xx",
    }
}

impl EncodeLabelValue for SessionType {
    fn encode(&self, encoder: &mut LabelValueEncoder) -> std::fmt::Result {
        encoder.write_str(match self {
            Self::Control => "control",
            Self::Vpn => "vpn",
        })
    }
}

impl EncodeLabelValue for Transport {
    fn encode(&self, encoder: &mut LabelValueEncoder) -> std::fmt::Result {
        encoder.write_str(match self {
            Self::Direct => "direct",
            Self::Relayed => "relayed",
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_required_metric_is_exposed_from_the_start() {
        let encoded = Metrics::new().encode();

        for line in [
            "twoviewer_host_agents_online 0",
            r#"twoviewer_sessions_active{type="control",transport="direct"} 0"#,
            r#"twoviewer_sessions_active{type="control",transport="relayed"} 0"#,
            r#"twoviewer_sessions_active{type="vpn",transport="direct"} 0"#,
            r#"twoviewer_sessions_active{type="vpn",transport="relayed"} 0"#,
            "twoviewer_relay_bytes_total 0",
            "twoviewer_sign_ins_failed_total 0",
            "twoviewer_migration_duration_seconds 0.0",
        ] {
            assert!(encoded.lines().any(|l| l == line), "{line}\n{encoded}");
        }
    }

    #[test]
    fn updates_are_exposed() {
        let metrics = Metrics::new();

        metrics.migration_duration.set(1.5);
        metrics
            .http_requests
            .get_or_create(&HttpLabels {
                route: "/devices/{id}".to_owned(),
                status_class: status_class(503),
            })
            .inc();

        let encoded = metrics.encode();
        assert!(encoded.contains("twoviewer_migration_duration_seconds 1.5\n"));
        assert!(encoded.contains(
            "twoviewer_http_requests_total{route=\"/devices/{id}\",status_class=\"5xx\"} 1\n"
        ));
    }

    #[test]
    fn status_classes() {
        for (status, class) in [
            (101, "1xx"),
            (204, "2xx"),
            (308, "3xx"),
            (404, "4xx"),
            (500, "5xx"),
        ] {
            assert_eq!(status_class(status), class);
        }
    }
}
