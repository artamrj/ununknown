use crate::{app::AppState, http::handlers};
use axum::{Json, Router, routing::get, routing::post};
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
        .route("/tracks/bulk/retry", post(handlers::retry_failed))
        .route("/tracks/bulk/skip", post(handlers::skip_review))
        .route("/path-template/preview", post(handlers::template_preview))
        .route("/apply/preview", post(handlers::apply_preview))
        .route("/apply/start", post(handlers::start_apply))
        .route("/apply/stop", post(handlers::stop_apply))
        .route("/events", get(handlers::events))
}
