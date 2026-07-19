mod app;
mod application;
mod config;
mod domain;
mod http;
mod infrastructure;
mod types;

use anyhow::{Context, Result, bail};
use axum::{Router, extract::DefaultBodyLimit, middleware};
use chrono::TimeZone;
use std::{
    net::SocketAddr,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use tokio::net::TcpListener;
use tower_http::{
    services::{ServeDir, ServeFile},
    trace::TraceLayer,
};
use tracing_subscriber::EnvFilter;

#[tokio::main]
async fn main() -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(EnvFilter::try_from_default_env().unwrap_or_else(|_| "info".into()))
        .init();

    let address = listen_address()?;
    let frontend = frontend_directory()?;

    let defaults = config::Config {
        db_path: std::env::var("UNUNKNOWN_DB")
            .unwrap_or_else(|_| ".local/cache/ununknown.sqlite".into()),
        input_dir: std::env::var("UNUNKNOWN_INPUT_DIR").unwrap_or_else(|_| ".local/input".into()),
        output_dir: std::env::var("UNUNKNOWN_OUTPUT_DIR")
            .unwrap_or_else(|_| ".local/output".into()),
        ..Default::default()
    };
    let pool = infrastructure::db::connect(&defaults.db_path).await?;
    let mut config = infrastructure::db::load_settings(&pool, defaults).await?;
    config.apply_environment_overrides();
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
    let api = http::router().layer(middleware::from_fn(http::protect_local_api));
    let app = Router::new().nest("/api", api);
    let app = if let Some(directory) = &frontend {
        app.fallback_service(
            ServeDir::new(directory)
                .append_index_html_on_directories(true)
                .not_found_service(ServeFile::new(directory.join("index.html"))),
        )
    } else {
        tracing::warn!("frontend build not found; API is available but the bundled UI is disabled");
        app
    };
    let app = app
        .with_state(state.clone())
        .layer(DefaultBodyLimit::max(256 * 1024))
        .layer(TraceLayer::new_for_http())
        .layer(middleware::from_fn(http::security_headers));

    tracing::info!(%address, frontend = ?frontend, "Ununknown is ready");
    axum::serve(TcpListener::bind(address).await?, app)
        .with_graceful_shutdown(shutdown_signal(state.clone()))
        .await?;
    state.pool.close().await;
    Ok(())
}

fn listen_address() -> Result<SocketAddr> {
    let raw = std::env::var("UNUNKNOWN_BIND").unwrap_or_else(|_| "127.0.0.1:7331".into());
    let address: SocketAddr = raw
        .parse()
        .with_context(|| format!("invalid UNUNKNOWN_BIND address: {raw}"))?;
    if !address.ip().is_loopback() {
        bail!("UNUNKNOWN_BIND must use a loopback address; this local API must not be exposed")
    }
    Ok(address)
}

fn frontend_directory() -> Result<Option<PathBuf>> {
    if let Some(configured) = std::env::var_os("UNUNKNOWN_STATIC_DIR") {
        let directory = PathBuf::from(configured);
        if !directory.join("index.html").is_file() {
            bail!(
                "UNUNKNOWN_STATIC_DIR does not contain index.html: {}",
                directory.display()
            );
        }
        return Ok(Some(directory));
    }

    let mut candidates = Vec::new();
    if let Ok(executable) = std::env::current_exe()
        && let Some(bin_directory) = executable.parent()
    {
        candidates.push(bin_directory.join("../share/ununknown"));
    }
    candidates.push(Path::new(env!("CARGO_MANIFEST_DIR")).join("frontend/dist"));
    Ok(candidates
        .into_iter()
        .find(|directory| directory.join("index.html").is_file()))
}

async fn shutdown_signal(state: Arc<app::AppState>) {
    let interrupt = async {
        if let Err(error) = tokio::signal::ctrl_c().await {
            tracing::error!(%error, "failed to install Ctrl+C handler");
        }
    };

    #[cfg(unix)]
    let terminate = async {
        match tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate()) {
            Ok(mut signal) => {
                signal.recv().await;
            }
            Err(error) => tracing::error!(%error, "failed to install SIGTERM handler"),
        }
    };

    #[cfg(unix)]
    tokio::select! {
        () = interrupt => {},
        () = terminate => {},
    }
    #[cfg(not(unix))]
    interrupt.await;

    tracing::info!("shutdown requested; stopping active workflow safely");
    state.cancel_workflow().await;
    let deadline = tokio::time::Instant::now() + Duration::from_secs(30);
    while state.workflow_running().await && tokio::time::Instant::now() < deadline {
        tokio::time::sleep(Duration::from_millis(100)).await;
    }
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
