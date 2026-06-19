use super::Candidate;
use crate::{
    domain::audio::AudioInfo,
    infrastructure::provider_cache::{ProviderCache, search_key},
};
use anyhow::Result;
use chrono::{Duration, Utc};
use reqwest::{Client, RequestBuilder};
use serde_json::Value;
use sqlx::SqlitePool;

pub async fn search(
    pool: &SqlitePool,
    client: &Client,
    api_key: &str,
    current: &AudioInfo,
) -> Result<Vec<Candidate>> {
    let Some(title) = current
        .title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(Vec::new());
    };
    let key = search_key(&format!(
        "{} {}",
        current.artist.as_deref().unwrap_or_default(),
        title
    ));
    let raw = if let Some(value) = ProviderCache::get(pool, "lastfm", &key).await? {
        value
    } else {
        let value = request_json(client.get("https://ws.audioscrobbler.com/2.0/").query(&[
            ("method", "track.search"),
            ("track", title),
            ("artist", current.artist.as_deref().unwrap_or_default()),
            ("api_key", api_key),
            ("format", "json"),
            ("limit", "5"),
        ]))
        .await?;
        ProviderCache::put(pool, "lastfm", &key, &value, Utc::now() + Duration::days(7)).await?;
        value
    };
    Ok(candidates_from_search(&raw))
}

fn candidates_from_search(raw: &Value) -> Vec<Candidate> {
    raw["results"]["trackmatches"]["track"]
        .as_array()
        .into_iter()
        .flatten()
        .take(5)
        .map(|track| {
            let title = track["name"].as_str().unwrap_or("Unknown Title").to_owned();
            let artist = track["artist"]
                .as_str()
                .unwrap_or("Unknown Artist")
                .to_owned();
            let listeners = track["listeners"]
                .as_str()
                .and_then(|value| value.parse::<f64>().ok())
                .unwrap_or_default();
            Candidate {
                provider: "lastfm".into(),
                title,
                artist,
                cover_url: track["image"]
                    .as_array()
                    .and_then(|images| {
                        images
                            .iter()
                            .rev()
                            .find_map(|image| image["#text"].as_str())
                    })
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned),
                recording_id: track["mbid"]
                    .as_str()
                    .filter(|value| !value.is_empty())
                    .map(str::to_owned),
                score_breakdown: Some(
                    serde_json::json!({
                        "provider_popularity": listeners,
                        "source": "lastfm_track_search"
                    })
                    .to_string(),
                ),
                raw_json: track.to_string(),
                ..Default::default()
            }
        })
        .collect()
}

async fn request_json(request: RequestBuilder) -> Result<Value> {
    Ok(request.send().await?.error_for_status()?.json().await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_lastfm_track_search() {
        let raw = serde_json::json!({
            "results": {"trackmatches": {"track": [
                {"name": "Song", "artist": "Artist", "listeners": "100", "mbid": "mbid"}
            ]}}
        });
        let candidates = candidates_from_search(&raw);
        assert_eq!(candidates[0].provider, "lastfm");
        assert_eq!(candidates[0].recording_id.as_deref(), Some("mbid"));
    }
}
