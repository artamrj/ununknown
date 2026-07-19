pub mod error;
pub mod handlers;
pub mod router;

pub use router::router;

use axum::{
    Json,
    extract::Request,
    http::{HeaderValue, Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};

/// Prevents arbitrary websites from using a browser to trigger the local write
/// API. Same-origin requests from the bundled UI, loopback development origins,
/// and native clients without an Origin header remain supported.
pub async fn protect_local_api(request: Request, next: Next) -> Response {
    let mutating = !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    if mutating
        && let Some(origin) = request.headers().get(header::ORIGIN)
        && !origin.to_str().ok().is_some_and(|origin| {
            let request_host = request
                .headers()
                .get(header::HOST)
                .and_then(|host| host.to_str().ok());
            origin_is_trusted(origin, request_host)
        })
    {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error":"cross-origin request rejected"})),
        )
            .into_response();
    }
    next.run(request).await
}

fn origin_is_trusted(origin: &str, request_host: Option<&str>) -> bool {
    let Ok(url) = reqwest::Url::parse(origin) else {
        return false;
    };
    if !matches!(url.scheme(), "http" | "https")
        || !url.username().is_empty()
        || url.password().is_some()
        || url.path() != "/"
        || url.query().is_some()
        || url.fragment().is_some()
    {
        return false;
    }
    let Some(origin_host) = url.host_str() else {
        return false;
    };

    if origin_host.eq_ignore_ascii_case("localhost")
        || origin_host
            .trim_matches(['[', ']'])
            .parse::<std::net::IpAddr>()
            .is_ok_and(|address| address.is_loopback())
    {
        return true;
    }

    let Some(request_host) = request_host else {
        return false;
    };
    let Ok(authority) = request_host.parse::<axum::http::uri::Authority>() else {
        return false;
    };
    let authority_host = authority.host().trim_matches(['[', ']']);
    let default_port = match url.scheme() {
        "http" => 80,
        "https" => 443,
        _ => return false,
    };

    origin_host
        .trim_matches(['[', ']'])
        .eq_ignore_ascii_case(authority_host)
        && url.port_or_known_default() == Some(authority.port_u16().unwrap_or(default_port))
}

pub async fn security_headers(request: Request, next: Next) -> Response {
    let is_api = request.uri().path().starts_with("/api/");
    let mut response = next.run(request).await;
    let headers = response.headers_mut();
    headers.insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static(
            "default-src 'self'; img-src 'self' data: https:; media-src 'self'; style-src 'self'; script-src 'self'; connect-src 'self'; object-src 'none'; base-uri 'none'; frame-ancestors 'none'",
        ),
    );
    headers.insert(
        header::X_CONTENT_TYPE_OPTIONS,
        HeaderValue::from_static("nosniff"),
    );
    headers.insert(
        header::REFERRER_POLICY,
        HeaderValue::from_static("no-referrer"),
    );
    headers.insert(
        header::HeaderName::from_static("permissions-policy"),
        HeaderValue::from_static("camera=(), microphone=(), geolocation=()"),
    );
    headers.insert(
        header::HeaderName::from_static("cross-origin-resource-policy"),
        HeaderValue::from_static("same-origin"),
    );
    if is_api {
        headers.insert(header::CACHE_CONTROL, HeaderValue::from_static("no-store"));
    }
    response
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::{Router, body::Body, middleware, routing::put};
    use tower::ServiceExt;

    #[test]
    fn loopback_browser_origins_are_trusted_for_local_development() {
        assert!(origin_is_trusted("http://localhost:5173", None));
        assert!(origin_is_trusted("http://127.0.0.1:7331", None));
        assert!(origin_is_trusted("http://[::1]:7331", None));
    }

    #[test]
    fn same_host_nas_browser_origin_is_trusted() {
        assert!(origin_is_trusted(
            "http://192.168.1.10:7331",
            Some("192.168.1.10:7331")
        ));
        assert!(origin_is_trusted(
            "https://luna-nas.example:8443",
            Some("luna-nas.example:8443")
        ));
        assert!(origin_is_trusted(
            "https://luna-nas.example",
            Some("luna-nas.example")
        ));
    }

    #[test]
    fn cross_origin_browser_requests_are_rejected() {
        assert!(!origin_is_trusted(
            "https://example.com",
            Some("192.168.1.10:7331")
        ));
        assert!(!origin_is_trusted(
            "http://192.168.1.20:7331",
            Some("192.168.1.10:7331")
        ));
        assert!(!origin_is_trusted(
            "http://192.168.1.10:8080",
            Some("192.168.1.10:7331")
        ));
        assert!(!origin_is_trusted("https://example.com", None));
        assert!(!origin_is_trusted("null", Some("192.168.1.10:7331")));
    }

    #[tokio::test]
    async fn middleware_allows_nas_ui_and_rejects_another_site() {
        let app = Router::new()
            .route("/api/test", put(|| async { StatusCode::NO_CONTENT }))
            .layer(middleware::from_fn(protect_local_api));
        let request = |origin: &'static str| {
            Request::builder()
                .method(Method::PUT)
                .uri("/api/test")
                .header(header::HOST, "192.168.1.10:7331")
                .header(header::ORIGIN, origin)
                .body(Body::empty())
                .unwrap()
        };

        let allowed = app
            .clone()
            .oneshot(request("http://192.168.1.10:7331"))
            .await
            .unwrap();
        assert_eq!(allowed.status(), StatusCode::NO_CONTENT);

        let rejected = app.oneshot(request("https://example.com")).await.unwrap();
        assert_eq!(rejected.status(), StatusCode::FORBIDDEN);
    }
}
