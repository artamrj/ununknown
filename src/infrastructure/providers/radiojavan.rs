use super::Candidate;
use crate::infrastructure::provider_cache::{ProviderCache, search_key};
use anyhow::{Context, Result, bail};
use chrono::{Duration, NaiveDate, Utc};
use percent_encoding::percent_decode_str;
use reqwest::Client;
use serde_json::Value;
use sqlx::SqlitePool;
use std::collections::HashSet;

const PROVIDER: &str = "radiojavan";
const API_BASE: &str = "https://rj-deskcloud.com/api2";
const RJ_USER_AGENT: &str = "Radio Javan/4.0.2 Ununknown/0.6";

pub async fn search(
    pool: &SqlitePool,
    client: &Client,
    title: &str,
    artist: Option<&str>,
) -> Result<Vec<Candidate>> {
    let query = match artist.filter(|value| !value.trim().is_empty()) {
        Some(artist) => format!("{} {}", artist.trim(), title.trim()),
        None => title.trim().to_owned(),
    };
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let key = search_key(&query);
    let raw = if let Some(value) = ProviderCache::get(pool, PROVIDER, &key).await? {
        value
    } else {
        let value = client
            .get(format!("{API_BASE}/search"))
            .header("Accept", "application/json")
            .header("x-rj-user-agent", RJ_USER_AGENT)
            .query(&[("query", query.as_str())])
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        reject_api_error(&value)?;
        ProviderCache::put(pool, PROVIDER, &key, &value, Utc::now() + Duration::days(7)).await?;
        value
    };
    Ok(parse_search_results(&raw))
}

pub async fn lookup_url(pool: &SqlitePool, client: &Client, url: &str) -> Result<Candidate> {
    let slug = song_slug(url)?;
    let key = format!("song:{}", slug.to_ascii_lowercase());
    let raw = if let Some(value) = ProviderCache::get(pool, PROVIDER, &key).await? {
        value
    } else {
        let value = client
            .get(format!("{API_BASE}/mp3"))
            .header("Accept", "application/json")
            .header("x-rj-user-agent", RJ_USER_AGENT)
            .query(&[("id", slug.as_str())])
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        reject_api_error(&value)?;
        ProviderCache::put(
            pool,
            PROVIDER,
            &key,
            &value,
            Utc::now() + Duration::days(14),
        )
        .await?;
        value
    };
    let track = raw
        .get("mp3")
        .filter(|value| value.is_object())
        .or_else(|| raw.get("data").filter(|value| value.is_object()))
        .unwrap_or(&raw);
    let mut candidate = candidate_from_track(track)
        .ok_or_else(|| anyhow::anyhow!("Radio Javan returned no usable song metadata"))?;
    candidate.score = 96.0;
    candidate.score_breakdown = Some(
        serde_json::json!({
            "source": "radiojavan_user_source_url",
            "sources": ["Radio Javan"],
            "user_verified_source": true,
            "final_score": 96.0
        })
        .to_string(),
    );
    Ok(candidate)
}

fn reject_api_error(raw: &Value) -> Result<()> {
    if let Some(message) = raw["error"]
        .as_str()
        .or_else(|| raw["error"]["message"].as_str())
        .or_else(|| {
            raw["message"]
                .as_str()
                .filter(|_| raw["status"].as_str() == Some("error"))
        })
    {
        bail!("Radio Javan API error: {message}");
    }
    Ok(())
}

fn song_slug(url: &str) -> Result<String> {
    let parsed = reqwest::Url::parse(url).context("invalid Radio Javan URL")?;
    if parsed.scheme() != "https"
        || !matches!(
            parsed.host_str(),
            Some("play.radiojavan.com" | "www.play.radiojavan.com")
        )
    {
        bail!("only HTTPS Radio Javan player links are supported");
    }
    let encoded = parsed
        .path()
        .strip_prefix("/song/")
        .filter(|value| !value.is_empty() && !value.contains('/'))
        .ok_or_else(|| anyhow::anyhow!("Radio Javan URL must point to a song"))?;
    Ok(percent_decode_str(encoded)
        .decode_utf8()
        .context("Radio Javan song URL is not valid UTF-8")?
        .into_owned())
}

fn parse_search_results(raw: &Value) -> Vec<Candidate> {
    let mut tracks = raw["mp3s"]
        .as_array()
        .into_iter()
        .flatten()
        .collect::<Vec<_>>();
    if raw["top"]["type"].as_str() == Some("mp3") {
        tracks.push(&raw["top"]);
    } else if raw["top"]["mp3"].is_object() {
        tracks.push(&raw["top"]["mp3"]);
    }
    let mut seen = HashSet::new();
    tracks
        .into_iter()
        .filter_map(candidate_from_track)
        .filter(|candidate| {
            let key = format!(
                "{}|{}",
                candidate.title.to_ascii_lowercase(),
                candidate.artist.to_ascii_lowercase()
            );
            seen.insert(key)
        })
        .take(10)
        .collect()
}

fn candidate_from_track(track: &Value) -> Option<Candidate> {
    let title = nonempty(track["song"].as_str())?;
    let artist = nonempty(track["artist"].as_str())?;
    let release_date = parse_release_date(track);
    let album = track["album"]
        .as_str()
        .or_else(|| track["album"]["album"].as_str())
        .or_else(|| track["album"]["title"].as_str())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned);
    Some(Candidate {
        provider: PROVIDER.into(),
        title: title.to_owned(),
        artist: artist.to_owned(),
        album,
        year: release_date
            .as_deref()
            .and_then(|date| date.get(..4))
            .map(str::to_owned),
        release_date,
        cover_url: track["photo"]
            .as_str()
            .or_else(|| track["thumbnail"].as_str())
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(str::to_owned),
        duration_delta: track["duration"].as_f64(),
        raw_json: track.to_string(),
        ..Default::default()
    })
}

fn parse_release_date(track: &Value) -> Option<String> {
    if let Some(created) = track["created_at"].as_str()
        && created.len() >= 10
        && NaiveDate::parse_from_str(&created[..10], "%Y-%m-%d").is_ok()
    {
        return Some(created[..10].to_owned());
    }
    track["date"]
        .as_str()
        .and_then(|date| NaiveDate::parse_from_str(date, "%b %d, %Y").ok())
        .map(|date| date.format("%Y-%m-%d").to_string())
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn example_track() -> Value {
        serde_json::json!({
            "id": 100168,
            "artist": "DJ Sasha",
            "song": "Tehraan Kenaret (Remix)",
            "photo": "https://assets.rjassets.com/static/mp3/dj-sasha-tehraan-kenaret-(remix)/74f18a0fed173b7.jpg",
            "duration": 183.798,
            "created_at": "2021-08-31T05:40:24-04:00",
            "album": null,
            "permlink": "DJ-Sasha-Tehraan-Kenaret-(Remix)"
        })
    }

    #[test]
    fn parses_search_metadata_and_original_artwork() {
        let candidates = parse_search_results(&serde_json::json!({"mp3s": [example_track()]}));
        assert_eq!(candidates.len(), 1);
        let candidate = &candidates[0];
        assert_eq!(candidate.provider, "radiojavan");
        assert_eq!(candidate.title, "Tehraan Kenaret (Remix)");
        assert_eq!(candidate.artist, "DJ Sasha");
        assert_eq!(candidate.year.as_deref(), Some("2021"));
        assert_eq!(candidate.release_date.as_deref(), Some("2021-08-31"));
        assert_eq!(candidate.duration_delta, Some(183.798));
        assert!(candidate.album.is_none());
        assert!(
            candidate
                .cover_url
                .as_deref()
                .is_some_and(|url| url.starts_with("https://assets.rjassets.com/"))
        );
    }

    #[test]
    fn extracts_literal_and_encoded_song_slugs() {
        assert_eq!(
            song_slug("https://play.radiojavan.com/song/dj-sasha-tehraan-kenaret-(remix)").unwrap(),
            "dj-sasha-tehraan-kenaret-(remix)"
        );
        assert_eq!(
            song_slug("https://play.radiojavan.com/song/dj-sasha-tehraan-kenaret-%28remix%29")
                .unwrap(),
            "dj-sasha-tehraan-kenaret-(remix)"
        );
    }

    #[test]
    fn rejects_non_song_and_non_radiojavan_urls() {
        assert!(song_slug("https://play.radiojavan.com/artist/dj-sasha").is_err());
        assert!(song_slug("https://example.com/song/dj-sasha").is_err());
    }
}
