//! The public listener: the API and, later, the web client.
//!
//! Every request gets a correlation ID (MNT-03). It is returned in the `x-request-id` header,
//! carried by every log line written while handling the request, and available to handlers
//! through the [`CorrelationId`] extractor for the audit events they record. Every answered
//! request is counted by route and status class (MNT-04).

use std::{sync::Arc, time::Instant};

use axum::{
    Router,
    extract::{FromRequestParts, MatchedPath, Request, State},
    http::{HeaderName, HeaderValue, StatusCode, request::Parts},
    middleware::{self, Next},
    response::Response,
};
use sea_orm::prelude::Uuid;
use tracing::{Instrument, info, info_span};

use crate::metrics::{HttpLabels, Metrics, status_class};

/// The response header that carries the correlation ID.
pub const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// The ID that ties a request's logs and audit events together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CorrelationId(pub Uuid);

/// The routes of the public listener. Features add theirs here.
pub fn router(metrics: Arc<Metrics>) -> Router {
    instrument(Router::new(), metrics)
}

/// Adds correlation IDs, request logs and request metrics to every route of `router`, and to its
/// fallback.
fn instrument(router: Router, metrics: Arc<Metrics>) -> Router {
    router.layer(middleware::from_fn_with_state(metrics, track))
}

async fn track(State(metrics): State<Arc<Metrics>>, mut request: Request, next: Next) -> Response {
    // A new ID for every request. An ID sent by the client is ignored, so that clients can
    // neither inject text into logs nor pose as another request.
    let correlation_id = CorrelationId(Uuid::new_v4());
    request.extensions_mut().insert(correlation_id);
    let method = request.method().clone();
    // The template, never the requested path: paths and query strings can carry tokens.
    let route = request
        .extensions()
        .get::<MatchedPath>()
        .map_or("unmatched", MatchedPath::as_str)
        .to_owned();
    let span = info_span!("request", correlation_id = %correlation_id.0);
    let started = Instant::now();

    let mut response = next.run(request).instrument(span.clone()).await;

    let status = response.status().as_u16();
    span.in_scope(|| {
        info!(
            method = %method,
            route,
            status,
            elapsed_ms = started.elapsed().as_millis() as u64,
            "request completed"
        );
    });
    metrics
        .http_requests
        .get_or_create(&HttpLabels {
            route,
            status_class: status_class(status),
        })
        .inc();
    response.headers_mut().insert(
        REQUEST_ID,
        HeaderValue::try_from(correlation_id.0.to_string()).expect("a UUID is a valid header"),
    );
    response
}

impl<S: Send + Sync> FromRequestParts<S> for CorrelationId {
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, _: &S) -> Result<Self, Self::Rejection> {
        // Only missing if a route was added outside `instrument`.
        parts
            .extensions
            .get::<CorrelationId>()
            .copied()
            .ok_or(StatusCode::INTERNAL_SERVER_ERROR)
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io,
        sync::{Arc, Mutex},
    };

    use axum::{body::Body, routing::get};
    use tower::ServiceExt;
    use tracing_subscriber::fmt::MakeWriter;

    use super::*;
    use crate::{
        config::{Log, LogFormat},
        telemetry,
    };

    /// A router with test routes, and the metrics it counts into.
    fn app() -> (Router, Arc<Metrics>) {
        let metrics = Arc::new(Metrics::new());
        let routes = Router::new()
            .route(
                "/devices/{id}",
                get(|CorrelationId(id): CorrelationId| async move { id.to_string() }),
            )
            .route("/fail", get(|| async { StatusCode::INTERNAL_SERVER_ERROR }));
        (instrument(routes, metrics.clone()), metrics)
    }

    async fn send(router: &Router, uri: &str) -> Response {
        router
            .clone()
            .oneshot(Request::get(uri).body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    fn request_id(response: &Response) -> Uuid {
        response.headers()[REQUEST_ID]
            .to_str()
            .unwrap()
            .parse()
            .unwrap()
    }

    async fn body(response: Response) -> String {
        let bytes = axum::body::to_bytes(response.into_body(), usize::MAX)
            .await
            .unwrap();
        String::from_utf8(bytes.to_vec()).unwrap()
    }

    fn count(metrics: &Metrics, route: &str, status_class: &'static str) -> u64 {
        metrics
            .http_requests
            .get_or_create(&HttpLabels {
                route: route.to_owned(),
                status_class,
            })
            .get()
    }

    #[tokio::test]
    async fn every_response_carries_a_new_correlation_id() {
        let (router, _) = app();

        let first = send(&router, "/devices/1").await;
        let second = send(&router, "/devices/1").await;

        assert_ne!(request_id(&first), request_id(&second));
        // The handler sees the same ID as the client.
        let id = request_id(&first);
        assert_eq!(body(first).await, id.to_string());
    }

    #[tokio::test]
    async fn ids_sent_by_the_client_are_ignored() {
        let (router, _) = app();
        let forged = Uuid::from_u128(1).to_string();
        let request = Request::get("/devices/1")
            .header(REQUEST_ID, &forged)
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(request).await.unwrap();

        assert_ne!(request_id(&response).to_string(), forged);
    }

    #[tokio::test]
    async fn requests_are_counted_by_route_and_status_class() {
        let (router, metrics) = app();

        send(&router, "/devices/1").await;
        send(&router, "/devices/2").await;
        let not_found = send(&router, "/nothing/here").await;
        send(&router, "/fail").await;

        assert_eq!(not_found.status(), StatusCode::NOT_FOUND);
        request_id(&not_found);
        assert_eq!(count(&metrics, "/devices/{id}", "2xx"), 2);
        assert_eq!(count(&metrics, "unmatched", "4xx"), 1);
        assert_eq!(count(&metrics, "/fail", "5xx"), 1);
    }

    /// A log destination that tests can read.
    #[derive(Clone, Default)]
    struct Buffer(Arc<Mutex<Vec<u8>>>);

    impl io::Write for Buffer {
        fn write(&mut self, buf: &[u8]) -> io::Result<usize> {
            self.0.lock().unwrap().extend_from_slice(buf);
            Ok(buf.len())
        }

        fn flush(&mut self) -> io::Result<()> {
            Ok(())
        }
    }

    impl<'a> MakeWriter<'a> for Buffer {
        type Writer = Self;

        fn make_writer(&'a self) -> Self::Writer {
            self.clone()
        }
    }

    #[tokio::test]
    async fn request_logs_carry_the_correlation_id_but_not_the_path() {
        let (router, _) = app();
        let logs = Buffer::default();
        let config = Log {
            format: LogFormat::Json,
            filter: "info".to_owned(),
        };
        let _guard =
            tracing::subscriber::set_default(telemetry::subscriber(&config, logs.clone()).unwrap());

        let response = send(&router, "/devices/8d2c41?token=s3cr3t").await;

        let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
        let line: serde_json::Value = logs
            .lines()
            .map(|line| serde_json::from_str(line).unwrap())
            .find(|line: &serde_json::Value| line["fields"]["message"] == "request completed")
            .expect(&logs);
        assert_eq!(
            line["span"]["correlation_id"],
            request_id(&response).to_string()
        );
        assert_eq!(line["fields"]["route"], "/devices/{id}");
        assert_eq!(line["fields"]["status"], 200);
        assert!(!logs.contains("8d2c41"), "{logs}");
        assert!(!logs.contains("s3cr3t"), "{logs}");
    }
}
