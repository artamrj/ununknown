mod app;
mod application;
mod config;
mod domain;
mod http;
mod infrastructure;
mod types;

use anyhow::Result;
use axum::Router;
use chrono::TimeZone;
use std::{sync::Arc, time::Duration};
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
    infrastructure::db::run_daily_cache_cleanup_if_due(&pool).await?;
    infrastructure::db::enforce_media_cache_limit(&pool).await?;
    let state = Arc::new(app::AppState::new(config, pool));
    let maintenance_state = state.clone();
    tokio::spawn(async move {
        loop {
            tokio::time::sleep(duration_until_next_local_midnight()).await;
            while maintenance_state.workflow_running().await {
                tokio::time::sleep(Duration::from_secs(60)).await;
            }
            if let Err(error) =
                infrastructure::db::run_daily_cache_cleanup_if_due(&maintenance_state.pool).await
            {
                tracing::warn!(%error, "daily disposable cache cleanup failed");
            }
        }
    });
    let media_cache_state = state.clone();
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(60 * 60));
        interval.tick().await;
        loop {
            interval.tick().await;
            if media_cache_state.workflow_running().await {
                continue;
            }
            if let Err(error) =
                infrastructure::db::enforce_media_cache_limit(&media_cache_state.pool).await
            {
                tracing::warn!(%error, "media-analysis cache limit check failed");
            }
        }
    });
    let app = Router::new().nest("/api", http::router()).with_state(state);

    tracing::info!("Open http://localhost:5173");
    axum::serve(TcpListener::bind("127.0.0.1:7331").await?, app).await?;
    Ok(())
}

fn duration_until_next_local_midnight() -> Duration {
    let now = chrono::Local::now();
    let Some(tomorrow) = now.date_naive().succ_opt() else {
        return Duration::from_secs(24 * 60 * 60);
    };
    let Some(midnight) = tomorrow.and_hms_opt(0, 0, 0) else {
        return Duration::from_secs(24 * 60 * 60);
    };
    let Some(next_midnight) = chrono::Local.from_local_datetime(&midnight).earliest() else {
        return Duration::from_secs(24 * 60 * 60);
    };
    (next_midnight - now)
        .to_std()
        .unwrap_or_else(|_| Duration::from_secs(24 * 60 * 60))
}
