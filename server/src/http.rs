//! The HTTPS listener: the API and, later, the web client.
//!
//! Every request gets a correlation ID (MNT-03). It is returned in the `x-request-id` header,
//! carried by every log line written while handling the request, and available to handlers
//! through the [`CorrelationId`] extractor for the audit events they record. The [`ClientOrigin`]
//! extractor gives them where the request came from. Every answered request is counted by route
//! and status class (MNT-04).
//!
//! Every response tells browsers to use only HTTPS (SEC-01), and forbids other sites to frame
//! the web client and scripts from other origins to run in it (SEC-10).
//!
//! Requests that may change something, which is any but `GET`, `HEAD` and `OPTIONS`, are refused
//! with `403` (`cross_site_request`) when a browser says that another site started them
//! ([`refuse_cross_site`]). With the `SameSite=Strict` sign-in cookie ([`crate::accounts`]), this
//! keeps other sites from acting in a user's name (SEC-09), including sites that browsers count as
//! the same site, such as other subdomains, and requests without a JSON body, which need no CORS
//! preflight.

use std::{
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};

use axum::{
    Router,
    extract::{ConnectInfo, FromRequestParts, MatchedPath, Request, State},
    http::{HeaderMap, HeaderName, HeaderValue, Method, StatusCode, header, request::Parts},
    middleware::{self, Next},
    response::{IntoResponse, Response},
};
use sea_orm::prelude::Uuid;
use tracing::{Instrument, info, info_span};

use crate::{
    api::ApiError,
    audit,
    metrics::{HttpLabels, Metrics, status_class},
    user_agent,
};

/// The response header that carries the correlation ID.
pub const REQUEST_ID: HeaderName = HeaderName::from_static("x-request-id");

/// The ID that ties a request's logs and audit events together.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CorrelationId(pub Uuid);

/// Where a request came from, for its audit events (AUD-02): the client's IP address, and its
/// browser and operating system.
///
/// The IP address is that of the TCP connection. A load balancer in front of the server passes
/// TCP through rather than terminating TLS (`server/README.md`), so it is the client's. Headers
/// such as `X-Forwarded-For` are ignored: any client can send them.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ClientOrigin(pub audit::Origin);

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

/// The HTTPS listener's router: the routes of every feature, in `routes`, with correlation IDs,
/// request logs, request metrics, the security headers, and cross-site requests refused.
pub fn router(routes: Router, metrics: Arc<Metrics>, hsts_max_age: Duration) -> Router {
    let routes = routes.layer(middleware::from_fn(refuse_cross_site));
    secure(instrument(routes, metrics), hsts_max_age)
}

/// Refuses requests that may change something when a browser says that another site, or another
/// origin of this site, started them.
///
/// Browsers send `Sec-Fetch-Site` and `Origin` with such requests. A request without either
/// doesn't come from a browser, such as one from a Host Agent, and so carries no cookie that a
/// browser added on its own: no other site can have forged it.
async fn refuse_cross_site(request: Request, next: Next) -> Response {
    if may_change(request.method()) && !from_this_origin(request.headers()) {
        return ApiError::Forbidden {
            code: "cross_site_request",
        }
        .into_response();
    }
    next.run(request).await
}

/// Whether a request with `method` may change something. The others are safe (RFC 9110).
fn may_change(method: &Method) -> bool {
    !matches!(*method, Method::GET | Method::HEAD | Method::OPTIONS)
}

/// Whether the browser headers in `headers`, if any, say that a page of this origin sent the
/// request.
fn from_this_origin(headers: &HeaderMap) -> bool {
    let fetch_site = headers.get("sec-fetch-site");
    if fetch_site.is_some_and(|site| site != "same-origin") {
        return false;
    }
    !headers.contains_key(header::ORIGIN) || same_origin(headers)
}

/// Whether the request comes from a page served by this host, over HTTPS: its `Origin` is
/// `https://` and its `Host`.
pub fn same_origin(headers: &HeaderMap) -> bool {
    let header = |name| headers.get(name).and_then(|value| value.to_str().ok());
    let (Some(origin), Some(host)) = (header(header::ORIGIN), header(header::HOST)) else {
        return false;
    };
    origin
        .strip_prefix("https://")
        .is_some_and(|origin| origin.eq_ignore_ascii_case(host))
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

impl<S: Send + Sync> FromRequestParts<S> for ClientOrigin {
    type Rejection = StatusCode;

    async fn from_request_parts(parts: &mut Parts, state: &S) -> Result<Self, Self::Rejection> {
        // Only missing if the server is not served with connection information.
        let ConnectInfo(address) = ConnectInfo::<SocketAddr>::from_request_parts(parts, state)
            .await
            .map_err(|_| StatusCode::INTERNAL_SERVER_ERROR)?;
        let user_agent = parts
            .headers
            .get(header::USER_AGENT)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default();
        Ok(Self(audit::Origin {
            ip_address: address.ip().to_canonical(),
            client_description: user_agent::describe(user_agent),
        }))
    }
}

#[cfg(test)]
mod tests {
    use std::{
        io,
        sync::{Arc, Mutex},
    };

    use axum::{
        body::Body,
        routing::{get, post},
    };
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
    async fn the_client_origin_is_the_connection_and_the_user_agent() {
        use axum::extract::connect_info::MockConnectInfo;

        let router = Router::new()
            .route(
                "/",
                get(|ClientOrigin(origin): ClientOrigin| async move {
                    format!("{} {}", origin.ip_address, origin.client_description)
                }),
            )
            .layer(MockConnectInfo(SocketAddr::from((
                [0, 0, 0, 0, 0, 0xffff, 0xc000, 0x0201],
                443,
            ))));
        let request = Request::get("/")
            .header(
                header::USER_AGENT,
                "Mozilla/5.0 (X11; Linux x86_64; rv:133.0) Gecko/20100101 Firefox/133.0",
            )
            .header("x-forwarded-for", "198.51.100.9")
            .body(Body::empty())
            .unwrap();

        let response = router.oneshot(request).await.unwrap();

        // An IPv4 address mapped into IPv6 is stored as IPv4.
        assert_eq!(body(response).await, "192.0.2.1 Firefox on Linux");
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

    fn headers(origin: Option<&'static str>, host: Option<&'static str>) -> HeaderMap {
        let mut headers = HeaderMap::new();
        if let Some(origin) = origin {
            headers.insert(header::ORIGIN, HeaderValue::from_static(origin));
        }
        if let Some(host) = host {
            headers.insert(header::HOST, HeaderValue::from_static(host));
        }
        headers
    }

    #[test]
    fn only_pages_of_this_host_over_https_are_the_same_origin() {
        for (origin, host, same) in [
            (
                Some("https://twoviewer.example"),
                Some("twoviewer.example"),
                true,
            ),
            (Some("https://localhost:8443"), Some("localhost:8443"), true),
            (
                Some("https://TwoViewer.example"),
                Some("twoviewer.example"),
                true,
            ),
            (
                Some("https://evil.example"),
                Some("twoviewer.example"),
                false,
            ),
            (
                Some("http://twoviewer.example"),
                Some("twoviewer.example"),
                false,
            ),
            (
                Some("https://twoviewer.example:8443"),
                Some("twoviewer.example"),
                false,
            ),
            (
                Some("https://twoviewer.example.evil"),
                Some("twoviewer.example"),
                false,
            ),
            (Some("null"), Some("twoviewer.example"), false),
            (None, Some("twoviewer.example"), false),
            (Some("https://twoviewer.example"), None, false),
        ] {
            assert_eq!(
                same_origin(&headers(origin, host)),
                same,
                "origin {origin:?}, host {host:?}"
            );
        }
    }

    #[tokio::test]
    async fn requests_that_other_sites_start_cannot_change_anything() {
        let router = Router::new()
            .route("/", get(|| async { "read" }).post(|| async { "changed" }))
            .route(
                "/{id}",
                post(|| async { "changed" }).delete(|| async { "deleted" }),
            )
            .layer(middleware::from_fn(refuse_cross_site));
        let send = async |method: Method, uri: &str, browser: &[(&str, &'static str)]| {
            let mut request = Request::builder()
                .method(method)
                .uri(uri)
                .header(header::HOST, "twoviewer.example");
            for (name, value) in browser {
                request = request.header(*name, *value);
            }
            let response = router
                .clone()
                .oneshot(request.body(Body::empty()).unwrap())
                .await
                .unwrap();
            let status = response.status();
            (status, body(response).await)
        };
        let refused = (
            StatusCode::FORBIDDEN,
            r#"{"error":"cross_site_request"}"#.to_owned(),
        );

        for browser in [
            &[("origin", "https://evil.example")][..],
            &[("origin", "null")],
            &[("origin", "http://twoviewer.example")],
            &[("sec-fetch-site", "cross-site")],
            // Another subdomain, which counts as the same site for `SameSite` cookies.
            &[("sec-fetch-site", "same-site")],
            &[("sec-fetch-site", "none")],
            &[
                ("sec-fetch-site", "cross-site"),
                ("origin", "https://twoviewer.example"),
            ],
            &[
                ("sec-fetch-site", "same-origin"),
                ("origin", "https://evil.example"),
            ],
        ] {
            assert_eq!(
                send(Method::POST, "/", browser).await,
                refused,
                "{browser:?}"
            );
            assert_eq!(
                send(Method::DELETE, "/1", browser).await,
                refused,
                "{browser:?}"
            );
            // Reading is not refused: other sites cannot see the answer.
            assert_eq!(
                send(Method::GET, "/", browser).await.0,
                StatusCode::OK,
                "{browser:?}"
            );
        }
        for browser in [
            &[][..],
            &[("origin", "https://twoviewer.example")],
            &[("sec-fetch-site", "same-origin")],
            &[
                ("sec-fetch-site", "same-origin"),
                ("origin", "https://twoviewer.example"),
            ],
        ] {
            assert_eq!(
                send(Method::POST, "/", browser).await,
                (StatusCode::OK, "changed".to_owned()),
                "{browser:?}"
            );
            assert_eq!(
                send(Method::DELETE, "/1", browser).await,
                (StatusCode::OK, "deleted".to_owned()),
                "{browser:?}"
            );
        }
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
