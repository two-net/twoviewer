//! The HTTPS listener: the API and, later, the web client.
//!
//! Every request gets a correlation ID (MNT-03). It is returned in the `x-request-id` header,
//! carried by every log line written while handling the request, and available to handlers
//! through the [`CorrelationId`] extractor for the audit events they record. Every answered
//! request is counted by route and status class (MNT-04).
//!
//! Every response tells browsers to use only HTTPS (SEC-01), and forbids other sites to frame
//! the web client and scripts from other origins to run in it (SEC-10).

use std::{
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    Router,
    extract::{FromRequestParts, MatchedPath, Request, State},
    http::{HeaderName, HeaderValue, StatusCode, header, request::Parts},
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

/// What the web client may load and who may frame it (SEC-10):
///
/// - `frame-ancestors 'none'`: no site may frame it, which protects the consent prompt against
///   clickjacking.
/// - `script-src 'self'`: only scripts from the TwoViewer origin run, and no inline scripts or
///   event handlers. This also keeps out third-party tracking code (PRIV-05).
/// - `style-src 'unsafe-inline'`, because Angular adds component styles as `<style>` elements.
/// - `base-uri 'self'` for the `<base href="/">` of `index.html`.
///
/// `'self'` includes WebSocket connections to the same host.
const CONTENT_SECURITY_POLICY: HeaderValue = HeaderValue::from_static(
    "default-src 'self'; script-src 'self'; style-src 'self' 'unsafe-inline'; \
     object-src 'none'; base-uri 'self'; form-action 'self'; frame-ancestors 'none'",
);

/// The routes of the HTTPS listener. Features add theirs here.
pub fn router(metrics: Arc<Metrics>, hsts_max_age: Duration) -> Router {
    secure(instrument(Router::new(), metrics), hsts_max_age)
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

/// Adds the security headers to every response of `router`, and of its fallback. They replace
/// any that a handler set, so that the policy is in one place.
fn secure(router: Router, hsts_max_age: Duration) -> Router {
    // Without `preload`, which is hard to undo.
    let hsts = HeaderValue::try_from(format!(
        "max-age={}; includeSubDomains",
        hsts_max_age.as_secs()
    ))
    .expect("digits are a valid header");
    router.layer(middleware::map_response_with_state(
        hsts,
        add_security_headers,
    ))
}

async fn add_security_headers(State(hsts): State<HeaderValue>, mut response: Response) -> Response {
    let headers = response.headers_mut();
    headers.insert(header::STRICT_TRANSPORT_SECURITY, hsts);
    headers.insert(header::CONTENT_SECURITY_POLICY, CONTENT_SECURITY_POLICY);
    // For browsers that do not support `frame-ancestors`.
    headers.insert(header::X_FRAME_OPTIONS, HeaderValue::from_static("DENY"));
    // A response runs as a script only if it is declared as one.
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
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

    #[tokio::test]
    async fn every_response_carries_the_security_headers() {
        let (routes, _) = app();
        let router = secure(routes, Duration::from_secs(31_536_000));

        for uri in ["/devices/1", "/fail", "/nothing/here"] {
            let response = send(&router, uri).await;

            let headers = response.headers();
            assert_eq!(
                headers[header::STRICT_TRANSPORT_SECURITY],
                "max-age=31536000; includeSubDomains",
                "{uri}"
            );
            let csp = headers[header::CONTENT_SECURITY_POLICY].to_str().unwrap();
            for directive in ["frame-ancestors 'none'", "script-src 'self';"] {
                assert!(csp.contains(directive), "{uri}: {csp}");
            }
            assert_eq!(headers[header::X_FRAME_OPTIONS], "DENY", "{uri}");
            assert_eq!(headers[header::X_CONTENT_TYPE_OPTIONS], "nosniff", "{uri}");
        }
    }

    #[tokio::test]
    async fn handlers_cannot_weaken_the_security_headers() {
        let routes = Router::new().route(
            "/",
            get(|| async { ([(header::CONTENT_SECURITY_POLICY, "frame-ancestors *")], "") }),
        );
        let router = secure(routes, Duration::ZERO);

        let response = send(&router, "/").await;

        assert_eq!(
            response.headers()[header::CONTENT_SECURITY_POLICY],
            CONTENT_SECURITY_POLICY
        );
        assert_eq!(
            response.headers()[header::STRICT_TRANSPORT_SECURITY],
            "max-age=0; includeSubDomains"
        );
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

        // A test on another thread can register the span and event in `track` while this one
        // installs its subscriber, and so leave them disabled (a race in `tracing-core`). Once
        // that registration is over, rebuilding the interest cache enables them.
        let mut attempts = 0;
        let (line, logs) = loop {
            let response = send(&router, "/devices/8d2c41?token=s3cr3t").await;
            let id = request_id(&response).to_string();
            let logs = String::from_utf8(logs.0.lock().unwrap().clone()).unwrap();
            let line = logs
                .lines()
                .map(|line| serde_json::from_str::<serde_json::Value>(line).unwrap())
                .find(|line| {
                    line["fields"]["message"] == "request completed"
                        && line["span"]["correlation_id"] == id
                });
            attempts += 1;
            match line {
                Some(line) => break (line, logs),
                None if attempts == 100 => {
                    panic!("no request log with correlation ID {id}: {logs}")
                }
                None => {
                    tracing::callsite::rebuild_interest_cache();
                    tokio::time::sleep(Duration::from_millis(10)).await;
                }
            }
        };

        assert_eq!(line["fields"]["route"], "/devices/{id}");
        assert_eq!(line["fields"]["status"], 200);
        assert!(!logs.contains("8d2c41"), "{logs}");
        assert!(!logs.contains("s3cr3t"), "{logs}");
    }
}
