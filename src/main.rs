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
    let config = config::Config::load()?;
    let pool = db::connect(&config.db_path).await?;
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
