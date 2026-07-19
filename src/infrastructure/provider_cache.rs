use anyhow::Result;
use chrono::{DateTime, NaiveDateTime, Utc};
use serde_json::Value;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;

pub struct ProviderCache;

impl ProviderCache {
    pub async fn get(pool: &SqlitePool, provider: &str, key: &str) -> Result<Option<Value>> {
        let row: Option<(String, String)> = sqlx::query_as(
            "SELECT response_json, expires_at FROM provider_cache WHERE provider=? AND cache_key=?",
        )
        .bind(provider)
        .bind(key)
        .fetch_optional(pool)
        .await?;
        let Some((response_json, expires_at)) = row else {
            return Ok(None);
        };
        let expires_at = DateTime::parse_from_rfc3339(&expires_at)
            .map(|value| value.with_timezone(&Utc))
            .or_else(|_| {
                NaiveDateTime::parse_from_str(&expires_at, "%Y-%m-%d %H:%M:%S")
                    .map(|value| value.and_utc())
            })?;
        if expires_at <= Utc::now() {
            sqlx::query("DELETE FROM provider_cache WHERE provider=? AND cache_key=?")
                .bind(provider)
                .bind(key)
                .execute(pool)
                .await?;
            return Ok(None);
        }
        Ok(Some(serde_json::from_str(&response_json)?))
    }

    pub async fn put(
        pool: &SqlitePool,
        provider: &str,
        key: &str,
        response: &Value,
        expires_at: DateTime<Utc>,
    ) -> Result<()> {
        sqlx::query(
            "INSERT INTO provider_cache(provider,cache_key,response_json,expires_at) VALUES(?,?,?,?) \
             ON CONFLICT(provider,cache_key) DO UPDATE SET response_json=excluded.response_json, expires_at=excluded.expires_at",
        )
        .bind(provider)
        .bind(key)
        .bind(serde_json::to_string(response)?)
        .bind(expires_at.to_rfc3339())
        .execute(pool)
        .await?;
        Ok(())
    }
}

pub fn fingerprint_key(fingerprint: &str) -> String {
    format!("fingerprint:{}", sha256_hex(fingerprint.trim()))
}

pub fn recording_key(mbid: &str) -> String {
    format!("recording:{}", mbid.trim().to_ascii_lowercase())
}

pub fn search_key(query: &str) -> String {
    format!("search:{}", normalize_query(query))
}

pub fn release_key(release_id: &str) -> String {
    format!("release:{}", release_id.trim().to_ascii_lowercase())
}

fn sha256_hex(value: &str) -> String {
    hex::encode(Sha256::digest(value.as_bytes()))
}

fn normalize_query(query: &str) -> String {
    query
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
        .to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;
    use chrono::Duration;

    async fn test_pool() -> SqlitePool {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sqlite");
        let pool = crate::infrastructure::db::connect(path.to_str().unwrap())
            .await
            .unwrap();
        std::mem::forget(dir);
        pool
    }

    #[tokio::test]
    async fn provider_cache_hits_and_overwrites() {
        let pool = test_pool().await;
        ProviderCache::put(
            &pool,
            "musicbrainz",
            "recording:abc",
            &serde_json::json!({"title":"old"}),
            Utc::now() + Duration::days(1),
        )
        .await
        .unwrap();
        ProviderCache::put(
            &pool,
            "musicbrainz",
            "recording:abc",
            &serde_json::json!({"title":"new"}),
            Utc::now() + Duration::days(1),
        )
        .await
        .unwrap();
        let hit = ProviderCache::get(&pool, "musicbrainz", "recording:abc")
            .await
            .unwrap()
            .unwrap();
        assert_eq!(hit["title"], "new");
    }

    #[tokio::test]
    async fn provider_cache_ignores_expired_rows() {
        let pool = test_pool().await;
        ProviderCache::put(
            &pool,
            "musicbrainz",
            "recording:old",
            &serde_json::json!({"title":"old"}),
            Utc::now() - Duration::days(1),
        )
        .await
        .unwrap();
        assert!(
            ProviderCache::get(&pool, "musicbrainz", "recording:old")
                .await
                .unwrap()
                .is_none()
        );
    }

    #[test]
    fn cache_keys_are_stable() {
        assert_eq!(recording_key(" ABC "), "recording:abc");
        assert_eq!(
            search_key("Recording:Song   AND Artist:Me"),
            "search:recording:song and artist:me"
        );
        assert_eq!(release_key(" REL "), "release:rel");
        assert!(fingerprint_key("abc").starts_with("fingerprint:"));
    }
}
