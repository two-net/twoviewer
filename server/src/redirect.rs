//! The plain-HTTP listener: it only redirects to HTTPS (SEC-01).
//!
//! Every request, whatever its method and path, gets a permanent redirect to the same host, path
//! and query on the HTTPS listener. From then on, HSTS ([`crate::http`]) keeps browsers from
//! coming back here. Its requests are not logged or counted, because internet scanners send many
//! and none of them reach the application.

use axum::{
    Router,
    extract::{Request, State},
    http::{StatusCode, header, uri::Authority},
    response::{IntoResponse, Response},
};

/// Redirects every request to HTTPS on `public_port`.
pub fn router(public_port: u16) -> Router {
    Router::new().fallback(redirect).with_state(public_port)
}

async fn redirect(State(public_port): State<u16>, request: Request) -> Response {
    let Some(authority) = request
        .headers()
        .get(header::HOST)
        .and_then(|host| host.to_str().ok())
        .and_then(|host| host.parse::<Authority>().ok())
    else {
        return StatusCode::BAD_REQUEST.into_response();
    };
    // Without the port the request came in on, which is the plain-HTTP one.
    let host = authority.host();
    let location = match public_port {
        443 => format!("https://{host}"),
        port => format!("https://{host}:{port}"),
    };
    let path = request
        .uri()
        .path_and_query()
        .map_or("/", |path| path.as_str());
    (
        StatusCode::PERMANENT_REDIRECT,
        [(header::LOCATION, location + path)],
    )
        .into_response()
}

#[cfg(test)]
mod tests {
    use axum::{body::Body, http::Method};
    use tower::ServiceExt;

    use super::*;

    async fn send(public_port: u16, method: Method, host: Option<&str>, uri: &str) -> Response {
        let mut request = Request::builder().method(method).uri(uri);
        if let Some(host) = host {
            request = request.header(header::HOST, host);
        }
        router(public_port)
            .oneshot(request.body(Body::empty()).unwrap())
            .await
            .unwrap()
    }

    fn location(response: &Response) -> &str {
        assert_eq!(response.status(), StatusCode::PERMANENT_REDIRECT);
        response.headers()[header::LOCATION].to_str().unwrap()
    }

    #[tokio::test]
    async fn requests_are_redirected_to_the_same_host_path_and_query() {
        let response = send(443, Method::GET, Some("example.test:8080"), "/a/b?c=d&e").await;

        assert_eq!(location(&response), "https://example.test/a/b?c=d&e");
    }

    #[tokio::test]
    async fn redirects_point_to_the_public_port() {
        let response = send(8443, Method::GET, Some("localhost:8080"), "/").await;

        assert_eq!(location(&response), "https://localhost:8443/");
    }

    #[tokio::test]
    async fn ipv6_hosts_keep_their_brackets() {
        let response = send(443, Method::GET, Some("[::1]:8080"), "/devices").await;

        assert_eq!(location(&response), "https://[::1]/devices");
    }

    #[tokio::test]
    async fn every_method_is_redirected() {
        for method in [Method::POST, Method::PUT, Method::DELETE, Method::HEAD] {
            let response = send(443, method, Some("example.test"), "/api").await;

            assert_eq!(location(&response), "https://example.test/api");
        }
    }

    #[tokio::test]
    async fn requests_without_a_valid_host_are_rejected() {
        for host in [None, Some("exa mple.test"), Some("")] {
            let response = send(443, Method::GET, host, "/").await;

            assert_eq!(response.status(), StatusCode::BAD_REQUEST, "{host:?}");
        }
    }

    #[tokio::test]
    async fn redirects_carry_no_hsts() {
        // Browsers ignore HSTS over plain HTTP (RFC 6797).
        let response = send(443, Method::GET, Some("example.test"), "/").await;

        assert!(
            !response
                .headers()
                .contains_key(header::STRICT_TRANSPORT_SECURITY)
        );
    }
}
