use crate::config::Config;
use anyhow::Result;
use sqlx::{SqlitePool, sqlite::SqlitePoolOptions};
use std::path::Path;

pub async fn connect(path: &str) -> Result<SqlitePool> {
    if let Some(parent) = Path::new(path).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let url = format!("sqlite://{path}?mode=rwc");
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .connect(&url)
        .await?;
    sqlx::migrate!().run(&pool).await?;
    Ok(pool)
}

pub async fn load_settings(pool: &SqlitePool, defaults: Config) -> Result<Config> {
    let db_path = defaults.db_path.clone();
    let secret = defaults.acoustid_api_key.clone();
    let musicbrainz_user_agent = defaults.musicbrainz_user_agent.clone();
    let value: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key='config'")
        .fetch_optional(pool)
        .await?;
    let mut config = match value {
        Some(value) => serde_json::from_str(&value).unwrap_or(defaults),
        None => defaults,
    };
    config.db_path = db_path;
    config.acoustid_api_key = secret;
    config.musicbrainz_user_agent = musicbrainz_user_agent;
    save_settings(pool, &config).await?;
    Ok(config)
}

pub async fn save_settings(pool: &SqlitePool, config: &Config) -> Result<()> {
    sqlx::query(
        "INSERT INTO settings(key,value) VALUES('config',?) ON CONFLICT(key) DO UPDATE SET value=excluded.value",
    )
    .bind(serde_json::to_string(config)?)
    .execute(pool)
    .await?;
    Ok(())
}

pub async fn cleanup(pool: &SqlitePool, config: &Config) -> Result<()> {
    sqlx::query("UPDATE tracks SET stage='failed',status='provider_error',stage_message='Interrupted by restart',error=COALESCE(error,'Interrupted by backend restart') WHERE status='processing'")
        .execute(pool).await?;
    sqlx::query(
        "DELETE FROM tracks WHERE updated_at IS NOT NULL AND julianday(updated_at) < julianday('now', ?)",
    )
    .bind(format!("-{} days", config.workspace_retention_days))
    .execute(pool)
    .await?;
    sqlx::query("DELETE FROM jobs WHERE status!='running' AND updated_at < datetime('now', ?)")
        .bind(format!("-{} days", config.job_retention_days))
        .execute(pool)
        .await?;
    sqlx::query("UPDATE jobs SET status='failed',error='Interrupted by backend restart' WHERE status='running'")
        .execute(pool).await?;
    sqlx::query("DELETE FROM provider_cache WHERE expires_at < datetime('now')")
        .execute(pool)
        .await?;
    Ok(())
}
