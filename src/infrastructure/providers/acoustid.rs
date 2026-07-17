use crate::infrastructure::provider_cache::{ProviderCache, fingerprint_key};
use anyhow::{Result, bail};
use chrono::{Duration, Utc};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use sqlx::SqlitePool;

#[derive(Debug)]
pub struct Hit {
    pub score: f64,
    pub recording_id: String,
}
#[derive(Deserialize, Serialize)]
struct Response {
    status: String,
    error: Option<ApiError>,
    #[serde(default)]
    results: Vec<ResultItem>,
}
#[derive(Deserialize, Serialize)]
struct ApiError {
    message: String,
}
#[derive(Deserialize, Serialize)]
struct ResultItem {
    score: f64,
    recordings: Option<Vec<Recording>>,
}
#[derive(Deserialize, Serialize)]
struct Recording {
    id: String,
}

pub async fn lookup(
    pool: &SqlitePool,
    client: &Client,
    key: &str,
    fingerprint: &str,
    duration: f64,
) -> Result<Vec<Hit>> {
    let cache_key = fingerprint_key(fingerprint);
    let raw = if let Some(value) = ProviderCache::get(pool, "acoustid", &cache_key).await? {
        value
    } else {
        let value: Value = client
            .post("https://api.acoustid.org/v2/lookup")
            .form(&[
                ("client", key),
                ("meta", "recordings releases releasegroups tracks compress"),
                ("fingerprint", fingerprint),
                ("duration", &duration.round().to_string()),
            ])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        ProviderCache::put(
            pool,
            "acoustid",
            &cache_key,
            &value,
            Utc::now() + Duration::days(90),
        )
        .await?;
        value
    };
    hits_from_response(serde_json::from_value(raw)?)
}

fn hits_from_response(response: Response) -> Result<Vec<Hit>> {
    if response.status != "ok" {
        bail!(
            "AcoustID rejected the request: {}",
            response
                .error
                .map(|error| error.message)
                .unwrap_or_else(|| "unknown API error".into())
        );
    }
    Ok(response
        .results
        .into_iter()
        .flat_map(|r| {
            r.recordings
                .unwrap_or_default()
                .into_iter()
                .map(move |v| Hit {
                    score: r.score,
                    recording_id: v.id,
                })
        })
        .collect())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::provider_cache::{ProviderCache, fingerprint_key};

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
    async fn lookup_uses_cached_response_without_network() {
        let pool = test_pool().await;
        let fingerprint = "abc";
        ProviderCache::put(
            &pool,
            "acoustid",
            &fingerprint_key(fingerprint),
            &serde_json::json!({
                "status": "ok",
                "results": [
                    { "score": 0.9, "recordings": [{ "id": "recording-1" }] }
                ]
            }),
            Utc::now() + Duration::days(1),
        )
        .await
        .unwrap();

        let hits = lookup(&pool, &Client::new(), "key", fingerprint, 123.0)
            .await
            .unwrap();
        assert_eq!(hits.len(), 1);
        assert_eq!(hits[0].recording_id, "recording-1");
    }
}
