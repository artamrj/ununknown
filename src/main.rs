mod app;
mod application;
mod config;
mod domain;
mod http;
mod infrastructure;
mod jobs;
mod types;

use anyhow::Result;
use axum::Router;
use axum::extract::MatchedPath;
use axum::http::Request;
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;
use uuid::Uuid;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();
    let defaults = config::Config {
        db_path: std::env::var("UNUNKNOWN_DB").unwrap_or_else(|_| "/cache/ununknown.sqlite".into()),
        input_dir: std::env::var("UNUNKNOWN_INPUT_DIR").unwrap_or_else(|_| "/music/input".into()),
        output_dir: std::env::var("UNUNKNOWN_OUTPUT_DIR")
            .unwrap_or_else(|_| "/music/output".into()),
        ..Default::default()
    };
    let pool = infrastructure::db::connect(&defaults.db_path).await?;
    let config = infrastructure::db::load_settings(&pool, defaults).await?;
    infrastructure::db::cleanup(&pool, &config).await?;
    let state = Arc::new(app::AppState::new(config, pool));
    let app = Router::new()
        .nest("/api", http::router())
        .fallback_service(infrastructure::static_files::service())
        .layer(
            TraceLayer::new_for_http()
                .make_span_with(|request: &Request<axum::body::Body>| {
                    let matched_path = request
                        .extensions()
                        .get::<MatchedPath>()
                        .map(MatchedPath::as_str)
                        .unwrap_or("unmatched");
                    tracing::info_span!(
                        "http_request",
                        request_id = %Uuid::new_v4(),
                        method = %request.method(),
                        uri = %request.uri(),
                        matched_path = %matched_path,
                    )
                })
                .on_request(())
                .on_response(
                    |response: &axum::http::Response<axum::body::Body>,
                     latency: std::time::Duration,
                     _span: &tracing::Span| {
                        let latency_ms = latency.as_millis() as u64;
                        if response.status().is_server_error() {
                            tracing::warn!(
                                status = response.status().as_u16(),
                                latency_ms,
                                "HTTP response completed with server error"
                            );
                        } else {
                            tracing::debug!(
                                status = response.status().as_u16(),
                                latency_ms,
                                "HTTP response completed"
                            );
                        }
                    },
                )
                .on_failure(
                    |classification: tower_http::classify::ServerErrorsFailureClass,
                     latency: std::time::Duration,
                     _span: &tracing::Span| {
                        tracing::error!(
                            failure = %classification,
                            latency_ms = latency.as_millis() as u64,
                            "HTTP request failed"
                        );
                    },
                ),
        )
        .with_state(state);
    let listener = TcpListener::bind("0.0.0.0:7331").await?;
    tracing::info!("Ununknown listening on http://0.0.0.0:7331");
    axum::serve(listener, app).await?;
    Ok(())
}
