mod api;
mod audio;
mod config;
mod db;
mod fingerprint;
mod fs_scan;
mod jobs;
mod matcher;
mod path_templates;
mod providers;
mod static_files;
mod tag_writer;

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
        acoustid_api_key: std::env::var("UNUNKNOWN_ACOUSTID_API_KEY").unwrap_or_default(),
        musicbrainz_user_agent: std::env::var("UNUNKNOWN_MUSICBRAINZ_USER_AGENT")
            .unwrap_or_else(|_| "Ununknown/0.2.0 (https://github.com/artamrj/ununknown)".into()),
        ..Default::default()
    };
    let pool = db::connect(&defaults.db_path).await?;
    let config = db::load_settings(&pool, defaults).await?;
    db::cleanup(&pool, &config).await?;
    let state = Arc::new(api::AppState::new(config, pool));
    let app = Router::new()
        .nest("/api", api::router())
        .fallback_service(static_files::service())
        .layer(TraceLayer::new_for_http())
        .with_state(state);
    let listener = TcpListener::bind("0.0.0.0:7331").await?;
    tracing::info!("Ununknown listening on http://0.0.0.0:7331");
    axum::serve(listener, app).await?;
    Ok(())
}
