use crate::db::cache::{ProviderCache, search_key};
use anyhow::{Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{Duration, Utc};
use reqwest::Client;
use sqlx::SqlitePool;

use crate::media::tags::{ArtworkInfo, inspect_artwork};

pub async fn fetch(client: &Client, url: &str) -> Result<Vec<u8>> {
    let response = crate::net::get(client, url).await?.error_for_status()?;
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
    fetch_verified_url_cached(pool, client, url, false)
        .await
        .map(|(data, _)| data)
}

pub async fn fetch_verified_url_cached(
    pool: &SqlitePool,
    client: &Client,
    url: &str,
    trusted_exact_release: bool,
) -> Result<(Vec<u8>, ArtworkInfo)> {
    let key = search_key(url);
    if let Some(value) = ProviderCache::get(pool, "artwork-url", &key).await?
        && let Some(encoded) = value["data_base64"].as_str()
    {
        let data = STANDARD.decode(encoded)?;
        match inspect_artwork(&data, trusted_exact_release) {
            Ok(info) => return Ok((data, info)),
            Err(_) => ProviderCache::delete(pool, "artwork-url", &key).await?,
        }
    }
    let data = fetch(client, url).await?;
    let info = inspect_artwork(&data, trusted_exact_release)?;
    ProviderCache::put(
        pool,
        "artwork-url",
        &key,
        &serde_json::json!({
            "data_base64": STANDARD.encode(&data),
            "url": url,
            "sha256": info.sha256,
            "width": info.width,
            "height": info.height,
            "mime": info.mime
        }),
        Utc::now() + Duration::days(30),
    )
    .await?;
    Ok((data, info))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::db::cache::ProviderCache;

    async fn test_pool() -> SqlitePool {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sqlite");
        let pool = crate::db::connect(path.to_str().unwrap()).await.unwrap();
        std::mem::forget(dir);
        pool
    }

    #[tokio::test]
    async fn cached_cover_art_by_url_decodes_bytes() {
        let pool = test_pool().await;
        let expected = crate::media::tags::test_artwork_png();
        ProviderCache::put(
            &pool,
            "artwork-url",
            &search_key("http://127.0.0.1:1/should-not-be-called"),
            &serde_json::json!({ "data_base64": STANDARD.encode(&expected) }),
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
        assert_eq!(data, expected);
    }
}
