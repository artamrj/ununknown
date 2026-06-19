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
use std::sync::Arc;
use tokio::net::TcpListener;
use tower_http::trace::TraceLayer;
use tracing_subscriber::EnvFilter;

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
        acoustid_api_key: std::env::var("UNUNKNOWN_ACOUSTID_API_KEY").unwrap_or_default(),
        musicbrainz_user_agent: std::env::var("UNUNKNOWN_MUSICBRAINZ_USER_AGENT")
            .unwrap_or_else(|_| "Ununknown/0.5.0 (https://github.com/artamrj/ununknown)".into()),
        ..Default::default()
    };
    let pool = infrastructure::db::connect(&defaults.db_path).await?;
    let config = infrastructure::db::load_settings(&pool, defaults).await?;
    infrastructure::db::cleanup(&pool, &config).await?;
    let state = Arc::new(app::AppState::new(config, pool));
    let app = Router::new()
        .nest("/api", http::router())
        .fallback_service(infrastructure::static_files::service())
        .layer(TraceLayer::new_for_http())
        .with_state(state);
    let listener = TcpListener::bind("0.0.0.0:7331").await?;
    tracing::info!("Ununknown listening on http://0.0.0.0:7331");
    axum::serve(listener, app).await?;
    Ok(())
}
