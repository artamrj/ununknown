use crate::{app::AppState, http::handlers};
use axum::{
    Json, Router,
    extract::State,
    http::StatusCode,
    response::IntoResponse,
    routing::{get, post},
};
use std::sync::Arc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route("/health", get(health))
        .route("/setup", get(handlers::setup).put(handlers::update_setup))
        .route("/status", get(handlers::workspace))
        .route("/identify", post(handlers::start_scan))
        .route("/stop", post(handlers::stop_scan))
        .route("/tracks", get(handlers::list_tracks))
        .route(
            "/tracks/{id}",
            axum::routing::delete(handlers::remove_track),
        )
        .route("/tracks/retry-issues", post(handlers::retry_issues))
        .route("/tracks/auto-approve", post(handlers::auto_approve_review))
        .route("/tracks/{id}/audio", get(handlers::track_audio))
        .route("/source/resolve", post(handlers::resolve_source))
        .route("/tracks/{id}/choose", post(handlers::select_candidate))
        .route("/tracks/{id}/review", post(handlers::return_to_review))
        .route(
            "/tracks/{id}/manual",
            axum::routing::put(handlers::manual_candidate),
        )
        .route(
            "/tracks/{id}/artwork",
            axum::routing::put(handlers::update_artwork),
        )
        .route(
            "/tracks/{id}/artwork/preview",
            get(handlers::artwork_preview),
        )
        .route(
            "/tracks/{id}/artwork/original",
            get(handlers::original_artwork_preview),
        )
        .route(
            "/candidates/{id}/artwork/preview",
            get(handlers::candidate_artwork_preview),
        )
        .route("/write", post(handlers::start_apply))
        .fallback(api_not_found)
}

async fn health(State(state): State<Arc<AppState>>) -> impl IntoResponse {
    match sqlx::query_scalar::<_, i64>("SELECT 1")
        .fetch_one(&state.pool)
        .await
    {
        Ok(1) => (StatusCode::OK, Json(serde_json::json!({"status":"ok"}))),
        Ok(_) | Err(_) => (
            StatusCode::SERVICE_UNAVAILABLE,
            Json(serde_json::json!({"status":"unavailable"})),
        ),
    }
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
