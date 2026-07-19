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
/// API. Requests from the bundled UI, the Vite development proxy, and native
/// clients without an Origin header remain supported.
pub async fn protect_local_api(request: Request, next: Next) -> Response {
    let mutating = !matches!(
        *request.method(),
        Method::GET | Method::HEAD | Method::OPTIONS
    );
    if mutating
        && let Some(origin) = request.headers().get(header::ORIGIN)
        && !origin.to_str().ok().is_some_and(origin_is_local)
    {
        return (
            StatusCode::FORBIDDEN,
            Json(serde_json::json!({"error":"cross-origin request rejected"})),
        )
            .into_response();
    }
    next.run(request).await
}

fn origin_is_local(origin: &str) -> bool {
    reqwest::Url::parse(origin)
        .ok()
        .and_then(|url| url.host_str().map(str::to_owned))
        .is_some_and(|host| {
            host.eq_ignore_ascii_case("localhost")
                || host
                    .trim_matches(['[', ']'])
                    .parse::<std::net::IpAddr>()
                    .is_ok_and(|address| address.is_loopback())
        })
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

    #[test]
    fn only_loopback_browser_origins_are_trusted() {
        assert!(origin_is_local("http://localhost:5173"));
        assert!(origin_is_local("http://127.0.0.1:7331"));
        assert!(origin_is_local("http://[::1]:7331"));
        assert!(!origin_is_local("https://example.com"));
        assert!(!origin_is_local("null"));
    }
}
