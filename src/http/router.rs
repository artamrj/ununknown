use crate::{app::AppState, http::handlers};
use axum::{Json, Router, http::StatusCode, routing::get, routing::post};
use std::sync::Arc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/health",
            get(|| async { Json(serde_json::json!({"status":"ok"})) }),
        )
        .route("/setup", get(handlers::setup).put(handlers::update_setup))
        .route("/status", get(handlers::workspace))
        .route("/identify", post(handlers::start_scan))
        .route("/stop", post(handlers::stop_scan))
        .route("/tracks", get(handlers::list_tracks))
        .route("/tracks/{id}/choose", post(handlers::select_candidate))
        .route(
            "/tracks/{id}/manual",
            axum::routing::put(handlers::manual_candidate),
        )
        .route(
            "/tracks/{id}/artwork",
            axum::routing::put(handlers::update_artwork),
        )
        .route("/write", post(handlers::start_apply))
        .fallback(api_not_found)
}

async fn api_not_found() -> (StatusCode, Json<serde_json::Value>) {
    (
        StatusCode::NOT_FOUND,
        Json(serde_json::json!({"error":"API route not found"})),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{app::AppState, config::Config, infrastructure::db};
    use axum::body::{Body, to_bytes};
    use axum::http::{Request, StatusCode};
    use tower::ServiceExt;

    async fn test_state() -> Arc<AppState> {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("router.sqlite");
        let pool = db::connect(path.to_str().unwrap()).await.unwrap();
        std::mem::forget(dir);
        Arc::new(AppState::new(Config::default(), pool))
    }

    #[tokio::test]
    async fn unknown_api_route_returns_json_404() {
        let app = router().with_state(test_state().await);
        let response = app
            .oneshot(
                Request::builder()
                    .uri("/missing")
                    .body(Body::empty())
                    .unwrap(),
            )
            .await
            .unwrap();

        assert_eq!(response.status(), StatusCode::NOT_FOUND);
        let body = to_bytes(response.into_body(), usize::MAX).await.unwrap();
        let value: serde_json::Value = serde_json::from_slice(&body).unwrap();
        assert_eq!(value["error"], "API route not found");
    }
}
