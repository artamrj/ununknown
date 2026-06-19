use crate::{app::AppState, http::handlers};
use axum::{Json, Router, http::StatusCode, routing::get, routing::post};
use std::sync::Arc;

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/health",
            get(|| async { Json(serde_json::json!({"status":"ok"})) }),
        )
        .route(
            "/settings",
            get(handlers::settings).put(handlers::update_settings),
        )
        .route("/settings/reset", post(handlers::reset_settings))
        .route(
            "/settings/reset/{section}",
            post(handlers::reset_settings_section),
        )
        .route("/workspace/clear", post(handlers::clear_workspace))
        .route("/workspace", get(handlers::workspace))
        .route("/providers/acoustid/test", post(handlers::test_acoustid))
        .route(
            "/providers/musicbrainz/test",
            post(handlers::test_musicbrainz),
        )
        .route("/providers/status", get(handlers::provider_status))
        .route("/providers/{provider}/test", post(handlers::test_provider))
        .route("/scan/start", post(handlers::start_scan))
        .route("/scan/stop", post(handlers::stop_scan))
        .route("/jobs", get(handlers::list_jobs))
        .route("/jobs/{id}", get(handlers::get_job))
        .route("/tracks", get(handlers::list_tracks))
        .route("/tracks/{id}", get(handlers::get_track))
        .route("/tracks/{id}/candidates", get(handlers::candidates))
        .route(
            "/tracks/{id}/select-candidate",
            post(handlers::select_candidate),
        )
        .route("/artwork/current/{id}", get(handlers::current_artwork))
        .route("/artwork/proposed/{id}", get(handlers::proposed_artwork))
        .route(
            "/candidates/{id}",
            axum::routing::put(handlers::edit_candidate),
        )
        .route("/tracks/{id}/retry", post(handlers::retry_track))
        .route("/tracks/{id}/skip", post(handlers::skip_track))
        .route(
            "/tracks/{id}/keep-current",
            post(handlers::keep_current_track),
        )
        .route("/tracks/bulk/retry", post(handlers::retry_failed))
        .route("/tracks/bulk/skip", post(handlers::skip_review))
        .route("/path-template/preview", post(handlers::template_preview))
        .route("/apply/preview", post(handlers::apply_preview))
        .route("/apply/start", post(handlers::start_apply))
        .route("/apply/stop", post(handlers::stop_apply))
        .route("/events", get(handlers::events))
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
