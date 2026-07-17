use crate::config::Config;
use anyhow::Result;
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use std::{path::Path, str::FromStr, time::Duration};

pub async fn connect(path: &str) -> Result<SqlitePool> {
    if let Some(parent) = Path::new(path).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let url = format!("sqlite://{path}?mode=rwc");
    let options = SqliteConnectOptions::from_str(&url)?
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(30));
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(30))
        .connect_with(options)
        .await?;
    sqlx::raw_sql(SCHEMA).execute(&pool).await?;
    Ok(pool)
}

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS tracks (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  path TEXT NOT NULL UNIQUE,
  output_path TEXT,
  filename TEXT NOT NULL,
  format TEXT,
  bitrate INTEGER,
  duration REAL,
  current_title TEXT,
  current_artist TEXT,
  current_album TEXT,
  current_album_artist TEXT,
  current_track_number INTEGER,
  file_mtime INTEGER,
  file_size INTEGER,
  content_fingerprint TEXT,
  selected_candidate_id INTEGER,
  status TEXT NOT NULL DEFAULT 'new',
  error TEXT,
  is_missing INTEGER NOT NULL DEFAULT 0,
  first_seen_at TEXT NOT NULL,
  last_seen_at TEXT NOT NULL,
  last_scanned_at TEXT NOT NULL,
  last_applied_at TEXT,
  last_apply_run_id TEXT,
  stage TEXT NOT NULL DEFAULT 'discovered',
  stage_message TEXT,
  retry_count INTEGER NOT NULL DEFAULT 0,
  next_retry_at TEXT,
  updated_at TEXT
);

CREATE TABLE IF NOT EXISTS candidates (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  track_id INTEGER NOT NULL REFERENCES tracks(id) ON DELETE CASCADE,
  provider TEXT NOT NULL,
  title TEXT,
  artist TEXT,
  album TEXT,
  album_artist TEXT,
  track_number INTEGER,
  track_total INTEGER,
  disc_number INTEGER,
  disc_total INTEGER,
  year TEXT,
  genre TEXT,
  composer TEXT,
  label TEXT,
  isrc TEXT,
  cover_url TEXT,
  musicbrainz_recording_id TEXT,
  musicbrainz_release_id TEXT,
  musicbrainz_artist_id TEXT,
  musicbrainz_album_artist_id TEXT,
  score REAL NOT NULL,
  raw_json TEXT,
  release_country TEXT,
  release_date TEXT,
  release_type TEXT,
  release_secondary_types TEXT,
  is_compilation INTEGER NOT NULL DEFAULT 0,
  duration_delta REAL,
  score_breakdown TEXT
);

CREATE TABLE IF NOT EXISTS candidate_sources (
  id INTEGER PRIMARY KEY AUTOINCREMENT,
  candidate_id INTEGER NOT NULL REFERENCES candidates(id) ON DELETE CASCADE,
  provider TEXT NOT NULL,
  confidence REAL,
  reason_json TEXT,
  raw_json TEXT,
  created_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS provider_cache (
  provider TEXT NOT NULL,
  cache_key TEXT NOT NULL,
  response_json TEXT NOT NULL,
  expires_at TEXT NOT NULL,
  PRIMARY KEY(provider, cache_key)
);

CREATE TABLE IF NOT EXISTS fingerprint_cache (
  path TEXT PRIMARY KEY,
  file_size INTEGER NOT NULL,
  file_mtime INTEGER NOT NULL,
  fingerprint TEXT NOT NULL,
  duration REAL NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS replaygain_cache (
  path TEXT PRIMARY KEY,
  file_size INTEGER NOT NULL,
  file_mtime_ns INTEGER NOT NULL,
  track_gain_db REAL NOT NULL,
  track_peak REAL NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE INDEX IF NOT EXISTS candidates_track_id ON candidates(track_id);
CREATE INDEX IF NOT EXISTS tracks_stage ON tracks(stage);
CREATE INDEX IF NOT EXISTS candidate_sources_candidate_id ON candidate_sources(candidate_id);
"#;

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

pub async fn cleanup(pool: &SqlitePool, _config: &Config) -> Result<()> {
    sqlx::query("UPDATE tracks SET stage='failed',status='provider_error',stage_message='Interrupted by restart',error=COALESCE(error,'Interrupted by backend restart') WHERE status='processing'")
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
