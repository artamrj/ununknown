use crate::config::Config;
use anyhow::{Context, Result};
use chrono::{DateTime, Local, Utc};
use sqlx::{
    SqlitePool,
    sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous},
};
use std::{path::Path, str::FromStr, time::Duration};

const DAILY_CACHE_CLEANUP_KEY: &str = "last_disposable_cache_cleanup";
const MEDIA_CACHE_LIMIT_BYTES: u64 = 100 * 1024 * 1024;
static MIGRATOR: sqlx::migrate::Migrator = sqlx::migrate!("./migrations");

pub async fn connect(path: &str) -> Result<SqlitePool> {
    if let Some(parent) = Path::new(path).parent() {
        tokio::fs::create_dir_all(parent).await?;
    }
    let url = format!("sqlite://{path}?mode=rwc");
    let options = SqliteConnectOptions::from_str(&url)?
        .create_if_missing(true)
        .foreign_keys(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(30));
    let pool = SqlitePoolOptions::new()
        .max_connections(5)
        .acquire_timeout(Duration::from_secs(30))
        .connect_with(options)
        .await?;
    MIGRATOR
        .run(&pool)
        .await
        .context("database migration failed")?;
    // Keep the legacy declaration referenced until the next schema change
    // removes it; migration files are now the authoritative upgrade path.
    debug_assert!(SCHEMA.contains("CREATE TABLE IF NOT EXISTS tracks"));
    restrict_database_permissions(path).await?;
    Ok(pool)
}

#[cfg(unix)]
async fn restrict_database_permissions(path: &str) -> Result<()> {
    use std::os::unix::fs::PermissionsExt;

    tokio::fs::set_permissions(path, std::fs::Permissions::from_mode(0o600))
        .await
        .with_context(|| format!("could not protect database file {path}"))?;
    Ok(())
}

#[cfg(not(unix))]
async fn restrict_database_permissions(_path: &str) -> Result<()> {
    Ok(())
}

const SCHEMA: &str = r#"
PRAGMA foreign_keys = ON;

CREATE TABLE IF NOT EXISTS settings (
  key TEXT PRIMARY KEY,
  value TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS maintenance (
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

CREATE TABLE IF NOT EXISTS artwork_overrides (
  path TEXT PRIMARY KEY,
  title TEXT NOT NULL,
  artist TEXT NOT NULL,
  cover_url TEXT NOT NULL,
  updated_at TEXT NOT NULL
);

CREATE TABLE IF NOT EXISTS integrity_cache (
  path TEXT PRIMARY KEY,
  file_size INTEGER NOT NULL,
  file_mtime_ns INTEGER NOT NULL,
  is_healthy INTEGER NOT NULL,
  diagnostic TEXT,
  checked_at TEXT NOT NULL
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
        Some(value) => serde_json::from_str(&value).context("stored configuration is invalid")?,
        None => defaults,
    };
    config.db_path = db_path;
    config.normalize();
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

/// Removes reproducible web-provider responses once per local calendar day and
/// limits reusable media-analysis results to 100 MiB. User settings and workflow
/// data live in separate tables and are intentionally preserved.
pub async fn run_daily_cache_cleanup_if_due(pool: &SqlitePool) -> Result<bool> {
    let last_cleanup: Option<String> =
        sqlx::query_scalar("SELECT value FROM maintenance WHERE key=?")
            .bind(DAILY_CACHE_CLEANUP_KEY)
            .fetch_optional(pool)
            .await?;
    let now = Utc::now();
    let today = now.with_timezone(&Local).date_naive();
    let cleanup_due = last_cleanup
        .as_deref()
        .and_then(|value| DateTime::parse_from_rfc3339(value).ok())
        .is_none_or(|last| last.with_timezone(&Local).date_naive() < today);
    if !cleanup_due {
        return Ok(false);
    }

    let provider_rows_removed = sqlx::query("DELETE FROM provider_cache")
        .execute(pool)
        .await?
        .rows_affected();
    let media_rows_removed =
        enforce_media_cache_limit_with_limit(pool, MEDIA_CACHE_LIMIT_BYTES).await?;
    sqlx::query(
        "INSERT INTO maintenance(key,value) VALUES(?,?) \
         ON CONFLICT(key) DO UPDATE SET value=excluded.value",
    )
    .bind(DAILY_CACHE_CLEANUP_KEY)
    .bind(now.to_rfc3339())
    .execute(pool)
    .await?;

    if provider_rows_removed + media_rows_removed > 0 {
        // DELETE makes pages reusable, while VACUUM and the WAL checkpoint
        // return that space to the filesystem.
        compact(pool).await?;
    }
    tracing::info!(
        provider_rows_removed,
        media_rows_removed,
        "daily cache maintenance complete"
    );
    Ok(true)
}

/// Enforces the combined 100 MiB limit independently of the midnight provider
/// cache purge, so a large scan cannot leave analysis caches oversized all day.
pub async fn enforce_media_cache_limit(pool: &SqlitePool) -> Result<u64> {
    let removed = enforce_media_cache_limit_with_limit(pool, MEDIA_CACHE_LIMIT_BYTES).await?;
    if removed > 0 {
        compact(pool).await?;
        tracing::info!(removed, "media-analysis cache limit enforced");
    }
    Ok(removed)
}

async fn compact(pool: &SqlitePool) -> Result<()> {
    sqlx::query("VACUUM").execute(pool).await?;
    sqlx::query("PRAGMA wal_checkpoint(TRUNCATE)")
        .execute(pool)
        .await?;
    Ok(())
}

async fn enforce_media_cache_limit_with_limit(pool: &SqlitePool, limit_bytes: u64) -> Result<u64> {
    let entries: Vec<(String, String, i64)> = sqlx::query_as(
        "SELECT cache_kind,path,estimated_bytes FROM (
           SELECT 'fingerprint' AS cache_kind,path,updated_at AS cached_at,
             length(CAST(path AS BLOB)) + length(CAST(fingerprint AS BLOB))
             + length(CAST(updated_at AS BLOB)) + 32 AS estimated_bytes
           FROM fingerprint_cache
           UNION ALL
           SELECT 'integrity',path,checked_at,
             length(CAST(path AS BLOB)) + length(CAST(COALESCE(diagnostic,'') AS BLOB))
             + length(CAST(checked_at AS BLOB)) + 40
           FROM integrity_cache
           UNION ALL
           SELECT 'replaygain',path,updated_at,
             length(CAST(path AS BLOB)) + length(CAST(updated_at AS BLOB)) + 48
           FROM replaygain_cache
         ) ORDER BY cached_at ASC",
    )
    .fetch_all(pool)
    .await?;
    let mut total_bytes = entries
        .iter()
        .map(|(_, _, bytes)| (*bytes).max(0) as u64)
        .sum::<u64>();
    let mut removed = 0;

    for (cache_kind, path, bytes) in entries {
        if total_bytes <= limit_bytes {
            break;
        }
        let query = match cache_kind.as_str() {
            "fingerprint" => "DELETE FROM fingerprint_cache WHERE path=?",
            "integrity" => "DELETE FROM integrity_cache WHERE path=?",
            "replaygain" => "DELETE FROM replaygain_cache WHERE path=?",
            _ => continue,
        };
        removed += sqlx::query(query)
            .bind(path)
            .execute(pool)
            .await?
            .rows_affected();
        total_bytes = total_bytes.saturating_sub(bytes.max(0) as u64);
    }
    Ok(removed)
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

    #[tokio::test]
    async fn daily_cleanup_preserves_settings_and_media_analysis_caches() {
        let pool = test_pool().await;
        sqlx::query("INSERT INTO settings(key,value) VALUES('config','saved')")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO provider_cache(provider,cache_key,response_json,expires_at) VALUES('artwork-url','cover','{}',datetime('now','+30 days'))")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO fingerprint_cache(path,file_size,file_mtime,fingerprint,duration,updated_at) VALUES('/music/song.mp3',1,2,'fingerprint',3.0,datetime('now'))")
            .execute(&pool)
            .await
            .unwrap();

        assert!(run_daily_cache_cleanup_if_due(&pool).await.unwrap());
        assert_eq!(
            sqlx::query_scalar::<_, String>("SELECT value FROM settings WHERE key='config'")
                .fetch_one(&pool)
                .await
                .unwrap(),
            "saved"
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM provider_cache")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM fingerprint_cache")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn daily_cleanup_does_not_run_twice_within_one_day() {
        let pool = test_pool().await;
        assert!(run_daily_cache_cleanup_if_due(&pool).await.unwrap());
        sqlx::query("INSERT INTO provider_cache(provider,cache_key,response_json,expires_at) VALUES('musicbrainz','fresh','{}',datetime('now','+7 days'))")
            .execute(&pool)
            .await
            .unwrap();

        assert!(!run_daily_cache_cleanup_if_due(&pool).await.unwrap());
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM provider_cache")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn media_cache_limit_evicts_the_oldest_entries_across_tables() {
        let pool = test_pool().await;
        sqlx::query("INSERT INTO fingerprint_cache(path,file_size,file_mtime,fingerprint,duration,updated_at) VALUES('/old.mp3',1,2,?,3.0,'2026-01-01T00:00:00Z')")
            .bind("x".repeat(100))
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO integrity_cache(path,file_size,file_mtime_ns,is_healthy,diagnostic,checked_at) VALUES('/new.mp3',1,2,1,?,'2026-01-02T00:00:00Z')")
            .bind("y".repeat(100))
            .execute(&pool)
            .await
            .unwrap();

        assert_eq!(
            enforce_media_cache_limit_with_limit(&pool, 180)
                .await
                .unwrap(),
            1
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM fingerprint_cache")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM integrity_cache")
                .fetch_one(&pool)
                .await
                .unwrap(),
            1
        );
    }

    #[tokio::test]
    async fn new_databases_record_the_schema_migration() {
        let pool = test_pool().await;
        let versions: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(versions, vec![1, 2, 3]);
    }

    #[tokio::test]
    async fn legacy_database_is_adopted_by_the_migrator() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("legacy.sqlite");
        let url = format!("sqlite://{}?mode=rwc", path.display());
        let legacy = SqlitePoolOptions::new().connect(&url).await.unwrap();
        sqlx::raw_sql(SCHEMA).execute(&legacy).await.unwrap();
        legacy.close().await;

        let pool = connect(path.to_str().unwrap()).await.unwrap();
        let versions: Vec<i64> =
            sqlx::query_scalar("SELECT version FROM _sqlx_migrations ORDER BY version")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(versions, vec![1, 2, 3]);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT COUNT(*) FROM tracks")
                .fetch_one(&pool)
                .await
                .unwrap(),
            0
        );
    }
}
