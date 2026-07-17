mod app;
mod application;
mod config;
mod domain;
mod http;
mod infrastructure;
mod types;

use anyhow::Result;
use axum::Router;
use std::sync::Arc;
use tokio::net::TcpListener;
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let defaults = config::Config {
        db_path: std::env::var("UNUNKNOWN_DB")
            .unwrap_or_else(|_| ".local/cache/ununknown.sqlite".into()),
        input_dir: std::env::var("UNUNKNOWN_INPUT_DIR").unwrap_or_else(|_| ".local/input".into()),
        output_dir: std::env::var("UNUNKNOWN_OUTPUT_DIR")
            .unwrap_or_else(|_| ".local/output".into()),
        ..Default::default()
    };
    let pool = infrastructure::db::connect(&defaults.db_path).await?;
    let config = infrastructure::db::load_settings(&pool, defaults).await?;
    infrastructure::db::cleanup(&pool, &config).await?;
    let app = Router::new()
        .nest("/api", http::router())
        .with_state(Arc::new(app::AppState::new(config, pool)));

    tracing::info!("Open http://localhost:5173");
    axum::serve(TcpListener::bind("127.0.0.1:7331").await?, app).await?;
    Ok(())
}
