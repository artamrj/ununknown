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
    let value: Option<String> = sqlx::query_scalar("SELECT value FROM settings WHERE key='config'")
        .fetch_optional(pool)
        .await?;
    let mut config = match value {
        Some(value) => serde_json::from_str(&value).unwrap_or(defaults),
        None => defaults,
    };
    config.db_path = db_path;
    if config.metadata_sources.acoustid.api_key.is_empty() {
        config.metadata_sources.acoustid.api_key = config.acoustid_api_key.clone();
    }
    config.acoustid_api_key = config.metadata_sources.acoustid.api_key.clone();
    if config.metadata_sources.musicbrainz.user_agent.is_empty() {
        config.metadata_sources.musicbrainz.user_agent = config.musicbrainz_user_agent.clone();
    }
    config.musicbrainz_user_agent = config.metadata_sources.musicbrainz.user_agent.clone();
    if !config.metadata_sources.discogs.api_key.is_empty()
        || !config.metadata_sources.discogs.token.is_empty()
    {
        config.metadata_sources.discogs.enabled = true;
    }
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

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_pool() -> SqlitePool {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sqlite");
        let pool = connect(path.to_str().unwrap()).await.unwrap();
        std::mem::forget(dir);
        pool
    }

    #[tokio::test]
    async fn cleanup_removes_only_expired_provider_cache_rows() {
        let pool = test_pool().await;
        sqlx::query("INSERT INTO provider_cache(provider,cache_key,response_json,expires_at) VALUES('musicbrainz','expired','{}',datetime('now','-1 day'))")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO provider_cache(provider,cache_key,response_json,expires_at) VALUES('musicbrainz','fresh','{}',datetime('now','+1 day'))")
            .execute(&pool)
            .await
            .unwrap();

        cleanup(&pool, &Config::default()).await.unwrap();

        let keys: Vec<String> =
            sqlx::query_scalar("SELECT cache_key FROM provider_cache ORDER BY cache_key")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(keys, vec!["fresh"]);
    }
}
