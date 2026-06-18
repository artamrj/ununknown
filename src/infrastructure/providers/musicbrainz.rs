use super::Candidate;
use crate::infrastructure::provider_cache::{ProviderCache, recording_key, search_key};
use anyhow::{Result, bail};
use chrono::{Duration as ChronoDuration, Utc};
use reqwest::Client;
use reqwest::RequestBuilder;
use serde_json::Value;
use sqlx::SqlitePool;
use std::sync::OnceLock;
use tokio::sync::Mutex;
use tokio::time::{Duration, Instant, sleep};

static NEXT_REQUEST: OnceLock<Mutex<Instant>> = OnceLock::new();

pub async fn recording(
    pool: &SqlitePool,
    client: &Client,
    user_agent: &str,
    id: &str,
) -> Result<Candidate> {
    validate_user_agent(user_agent)?;
    let key = recording_key(id);
    let raw = if let Some(value) = ProviderCache::get(pool, "musicbrainz", &key).await? {
        value
    } else {
        let value = request_json(
            client
                .get(format!("https://musicbrainz.org/ws/2/recording/{id}"))
                .query(&[("fmt", "json"), ("inc", "artists+releases+isrcs")])
                .header("User-Agent", user_agent),
        )
        .await?;
        ProviderCache::put(
            pool,
            "musicbrainz",
            &key,
            &value,
            Utc::now() + ChronoDuration::days(30),
        )
        .await?;
        value
    };
    Ok(candidate_from_recording(&raw, id))
}

pub async fn search(
    pool: &SqlitePool,
    client: &Client,
    user_agent: &str,
    title: &str,
    artist: Option<&str>,
) -> Result<Vec<Candidate>> {
    validate_user_agent(user_agent)?;
    let query = match artist.filter(|value| !value.trim().is_empty()) {
        Some(artist) => format!("recording:\"{title}\" AND artist:\"{artist}\""),
        None => format!("recording:\"{title}\""),
    };
    let key = search_key(&query);
    let raw = if let Some(value) = ProviderCache::get(pool, "musicbrainz", &key).await? {
        value
    } else {
        let value = request_json(
            client
                .get("https://musicbrainz.org/ws/2/recording")
                .query(&[("fmt", "json"), ("limit", "3"), ("query", &query)])
                .header("User-Agent", user_agent),
        )
        .await?;
        ProviderCache::put(
            pool,
            "musicbrainz",
            &key,
            &value,
            Utc::now() + ChronoDuration::days(7),
        )
        .await?;
        value
    };
    Ok(raw["recordings"]
        .as_array()
        .into_iter()
        .flatten()
        .map(candidate_from_search)
        .collect())
}

fn candidate_from_recording(raw: &Value, id: &str) -> Candidate {
    let release = raw["releases"].as_array().and_then(|v| v.first());
    let artist = raw["artist-credit"].as_array().and_then(|v| v.first());
    let artist_obj = artist.and_then(|v| v.get("artist"));
    let release_id = release.and_then(|v| v["id"].as_str()).map(str::to_owned);
    Candidate {
        title: raw["title"].as_str().unwrap_or("Unknown Title").into(),
        artist: artist
            .and_then(|v| v["name"].as_str())
            .unwrap_or("Unknown Artist")
            .into(),
        album: release.and_then(|v| v["title"].as_str()).map(str::to_owned),
        year: release.and_then(|v| v["date"].as_str()).map(str::to_owned),
        isrc: raw["isrcs"]
            .as_array()
            .and_then(|v| v.first())
            .and_then(Value::as_str)
            .map(str::to_owned),
        recording_id: Some(id.into()),
        release_id: release_id.clone(),
        artist_id: artist_obj.and_then(|v| v["id"].as_str()).map(str::to_owned),
        cover_url: release_id.map(|v| format!("https://coverartarchive.org/release/{v}/front-500")),
        raw_json: raw.to_string(),
        ..Default::default()
    }
}

pub async fn test_connection(client: &Client, user_agent: &str) -> Result<()> {
    validate_user_agent(user_agent)?;
    request_json(
        client
            .get("https://musicbrainz.org/ws/2/recording")
            .query(&[
                ("fmt", "json"),
                ("limit", "1"),
                ("query", "recording:music"),
            ])
            .header("User-Agent", user_agent),
    )
    .await?;
    Ok(())
}

fn validate_user_agent(user_agent: &str) -> Result<()> {
    if !crate::config::Config::valid_musicbrainz_user_agent(user_agent) {
        bail!("MusicBrainz contact must include an email address or website");
    }
    Ok(())
}

async fn rate_limit() {
    let limiter = NEXT_REQUEST.get_or_init(|| Mutex::new(Instant::now()));
    let mut next = limiter.lock().await;
    let now = Instant::now();
    if *next > now {
        sleep(*next - now).await;
    }
    *next = Instant::now() + Duration::from_secs(1);
}

async fn request_json(request: RequestBuilder) -> Result<Value> {
    for attempt in 0..3 {
        rate_limit().await;
        let response = request
            .try_clone()
            .ok_or_else(|| anyhow::anyhow!("could not retry MusicBrainz request"))?
            .send()
            .await?;
        if (response.status().as_u16() == 429 || response.status().is_server_error()) && attempt < 2
        {
            sleep(Duration::from_secs(2_u64.pow(attempt))).await;
            continue;
        }
        return Ok(response.error_for_status()?.json().await?);
    }
    unreachable!()
}

fn candidate_from_search(raw: &Value) -> Candidate {
    let artist = raw["artist-credit"]
        .as_array()
        .and_then(|value| value.first());
    let release = raw["releases"].as_array().and_then(|value| value.first());
    let release_id = release
        .and_then(|value| value["id"].as_str())
        .map(str::to_owned);
    Candidate {
        title: raw["title"].as_str().unwrap_or("Unknown Title").into(),
        artist: artist
            .and_then(|value| value["name"].as_str())
            .unwrap_or("Unknown Artist")
            .into(),
        album: release
            .and_then(|value| value["title"].as_str())
            .map(str::to_owned),
        recording_id: raw["id"].as_str().map(str::to_owned),
        release_id: release_id.clone(),
        artist_id: artist
            .and_then(|value| value["artist"]["id"].as_str())
            .map(str::to_owned),
        cover_url: release_id
            .map(|id| format!("https://coverartarchive.org/release/{id}/front-500")),
        raw_json: raw.to_string(),
        ..Default::default()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::infrastructure::provider_cache::{ProviderCache, recording_key, search_key};
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
    async fn recording_uses_cached_response_without_network() {
        let pool = test_pool().await;
        ProviderCache::put(
            &pool,
            "musicbrainz",
            &recording_key("recording-1"),
            &serde_json::json!({
                "title": "Song",
                "artist-credit": [{ "name": "Artist", "artist": { "id": "artist-1" } }],
                "releases": [{ "id": "release-1", "title": "Album", "date": "2024" }],
                "isrcs": ["USRC17607839"]
            }),
            Utc::now() + Duration::days(1),
        )
        .await
        .unwrap();

        let candidate = recording(
            &pool,
            &Client::new(),
            "Ununknown/0.1 (test@example.com)",
            "recording-1",
        )
        .await
        .unwrap();
        assert_eq!(candidate.title, "Song");
        assert_eq!(candidate.release_id.as_deref(), Some("release-1"));
    }

    #[tokio::test]
    async fn search_uses_cached_response_without_network() {
        let pool = test_pool().await;
        let query = "recording:\"Song\" AND artist:\"Artist\"";
        ProviderCache::put(
            &pool,
            "musicbrainz",
            &search_key(query),
            &serde_json::json!({
                "recordings": [{
                    "id": "recording-1",
                    "title": "Song",
                    "artist-credit": [{ "name": "Artist", "artist": { "id": "artist-1" } }],
                    "releases": [{ "id": "release-1", "title": "Album" }]
                }]
            }),
            Utc::now() + Duration::days(1),
        )
        .await
        .unwrap();

        let candidates = search(
            &pool,
            &Client::new(),
            "Ununknown/0.1 (test@example.com)",
            "Song",
            Some("Artist"),
        )
        .await
        .unwrap();
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].recording_id.as_deref(), Some("recording-1"));
    }
}
