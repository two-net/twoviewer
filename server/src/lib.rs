pub mod accounts;
pub mod api;
pub mod app;
pub mod audit;
pub mod config;
pub mod db;
pub mod email;
pub mod http;
pub mod metrics;
pub mod monitoring;
pub mod realtime;
pub mod redirect;
pub mod telemetry;
pub mod throttle;
pub mod tls;
pub mod user_agent;

/// The product's name, as emails and authenticator apps show it. It is never translated (UI-01,
/// `brand/README.md`).
pub const PRODUCT_NAME: &str = "TwoViewer";
