use crate::infrastructure::provider_cache::{ProviderCache, release_key};
use anyhow::Result;
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{Duration, Utc};
use reqwest::Client;
use sqlx::SqlitePool;

pub async fn fetch(client: &Client, url: &str) -> Result<Vec<u8>> {
    Ok(client
        .get(url)
        .send()
        .await?
        .error_for_status()?
        .bytes()
        .await?
        .to_vec())
}

pub async fn fetch_cached(
    pool: &SqlitePool,
    client: &Client,
    release_id: &str,
    url: &str,
) -> Result<Vec<u8>> {
    let key = release_key(release_id);
    if let Some(value) = ProviderCache::get(pool, "coverart", &key).await? {
        if let Some(encoded) = value["data_base64"].as_str() {
            return Ok(STANDARD.decode(encoded)?);
        }
    }
    let data = fetch(client, url).await?;
    ProviderCache::put(
        pool,
        "coverart",
        &key,
        &serde_json::json!({ "data_base64": STANDARD.encode(&data) }),
        Utc::now() + Duration::days(30),
    )
    .await?;
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::provider_cache::{ProviderCache, release_key};

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
    async fn cached_cover_art_decodes_bytes() {
        let pool = test_pool().await;
        ProviderCache::put(
            &pool,
            "coverart",
            &release_key("release-1"),
            &serde_json::json!({ "data_base64": STANDARD.encode([1_u8, 2, 3]) }),
            Utc::now() + Duration::days(1),
        )
        .await
        .unwrap();

        let data = fetch_cached(
            &pool,
            &Client::new(),
            "release-1",
            "http://127.0.0.1:1/should-not-be-called",
        )
        .await
        .unwrap();
        assert_eq!(data, vec![1, 2, 3]);
    }
}
