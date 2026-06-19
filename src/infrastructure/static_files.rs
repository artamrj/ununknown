use tower_http::services::{ServeDir, ServeFile};

pub fn service() -> ServeDir<ServeFile> {
    service_from("frontend/dist")
}

pub fn service_from(path: impl AsRef<std::path::Path>) -> ServeDir<ServeFile> {
    let path = path.as_ref();
    ServeDir::new(path).fallback(ServeFile::new(path.join("index.html")))
}

#[cfg(test)]
mod tests {
    use super::*;
    use axum::Router;
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    #[tokio::test]
    async fn unknown_static_route_falls_back_to_index_html() {
        let dir = tempfile::tempdir().unwrap();
        tokio::fs::write(dir.path().join("index.html"), "<main>app</main>")
            .await
            .unwrap();

        let response = Router::new()
            .fallback_service(service_from(dir.path()))
            .oneshot(
                Request::builder()
                    .uri("/settings/metadata-sources")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::OK);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        assert_eq!(&body[..], b"<main>app</main>");
    }
}
