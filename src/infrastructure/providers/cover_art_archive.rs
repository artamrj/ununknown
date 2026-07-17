use crate::infrastructure::provider_cache::{ProviderCache, search_key};
use anyhow::{Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{Duration, Utc};
use reqwest::Client;
use sqlx::SqlitePool;

pub async fn fetch(client: &Client, url: &str) -> Result<Vec<u8>> {
    let response = crate::infrastructure::resilient_http::get(client, url)
        .await?
        .error_for_status()?;
    if response
        .content_length()
        .is_some_and(|length| length > 20 * 1024 * 1024)
    {
        bail!("cover image exceeds the 20 MB safety limit");
    }
    let bytes = response.bytes().await?;
    if bytes.len() > 20 * 1024 * 1024 {
        bail!("cover image exceeds the 20 MB safety limit");
    }
    Ok(bytes.to_vec())
}

pub async fn fetch_url_cached(pool: &SqlitePool, client: &Client, url: &str) -> Result<Vec<u8>> {
    let key = search_key(url);
    if let Some(value) = ProviderCache::get(pool, "artwork-url", &key).await?
        && let Some(encoded) = value["data_base64"].as_str()
    {
        return Ok(STANDARD.decode(encoded)?);
    }
    let data = fetch(client, url).await?;
    ProviderCache::put(
        pool,
        "artwork-url",
        &key,
        &serde_json::json!({"data_base64": STANDARD.encode(&data), "url": url}),
        Utc::now() + Duration::days(30),
    )
    .await?;
    Ok(data)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::provider_cache::ProviderCache;

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
    async fn cached_cover_art_by_url_decodes_bytes() {
        let pool = test_pool().await;
        ProviderCache::put(
            &pool,
            "artwork-url",
            &search_key("http://127.0.0.1:1/should-not-be-called"),
            &serde_json::json!({ "data_base64": STANDARD.encode([1_u8, 2, 3]) }),
            Utc::now() + Duration::days(1),
        )
        .await
        .unwrap();

        let data = fetch_url_cached(
            &pool,
            &Client::new(),
            "http://127.0.0.1:1/should-not-be-called",
        )
        .await
        .unwrap();
        assert_eq!(data, vec![1, 2, 3]);
    }
}
