use super::Candidate;
use crate::infrastructure::provider_cache::{ProviderCache, search_key};
use anyhow::{Context, Result, bail};
use chrono::{Duration, Utc};
use reqwest::{Client, RequestBuilder};
use serde_json::Value;
use sqlx::SqlitePool;

const PROVIDER: &str = "genius";
const WEB_API_BASE: &str = "https://genius.com/api";
const BROWSER_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/138 Safari/537.36";

pub async fn search(
    pool: &SqlitePool,
    client: &Client,
    title: &str,
    artist: Option<&str>,
) -> Result<Vec<Candidate>> {
    let query = search_query(title, artist);
    if query.is_empty() {
        return Ok(Vec::new());
    }
    let raw = search_raw(pool, client, &query).await?;
    let results = search_results(&raw);
    let detail = match results.first().and_then(|result| result["id"].as_i64()) {
        Some(id) => song_detail(pool, client, id).await.ok(),
        None => None,
    };
    let mut candidates = results
        .into_iter()
        .take(5)
        .filter_map(candidate_from_song)
        .collect::<Vec<_>>();
    if let Some(detail) = detail
        && let Some(candidate) = candidate_from_song(&detail["response"]["song"])
    {
        if let Some(first) = candidates.first_mut() {
            *first = candidate;
        } else {
            candidates.push(candidate);
        }
    }
    Ok(candidates)
}

pub async fn lookup_url(pool: &SqlitePool, client: &Client, url: &str) -> Result<Candidate> {
    let path = song_path(url)?;
    let query = path
        .trim_start_matches('/')
        .strip_suffix("-lyrics")
        .unwrap_or_default()
        .replace('-', " ");
    let raw = search_raw(pool, client, &query).await?;
    let result = search_results(&raw)
        .into_iter()
        .find(|result| {
            result["url"]
                .as_str()
                .and_then(|url| song_path(url).ok())
                .is_some_and(|candidate_path| candidate_path.eq_ignore_ascii_case(&path))
        })
        .ok_or_else(|| anyhow::anyhow!("Genius could not resolve that song page"))?;
    let id = result["id"]
        .as_i64()
        .ok_or_else(|| anyhow::anyhow!("Genius returned a song without an ID"))?;
    let detail = song_detail(pool, client, id).await?;
    let song = &detail["response"]["song"];
    let mut candidate = candidate_from_song(song)
        .ok_or_else(|| anyhow::anyhow!("Genius returned no usable song metadata"))?;
    candidate.score = 96.0;
    candidate.score_breakdown = Some(
        serde_json::json!({
            "source": "genius_user_source_url",
            "sources": ["Genius"],
            "user_verified_source": true,
            "final_score": 96.0
        })
        .to_string(),
    );
    Ok(candidate)
}

fn search_query(title: &str, artist: Option<&str>) -> String {
    match artist.map(str::trim).filter(|artist| !artist.is_empty()) {
        Some(artist) => format!("{artist} {}", title.trim()),
        None => title.trim().to_owned(),
    }
}

async fn search_raw(pool: &SqlitePool, client: &Client, query: &str) -> Result<Value> {
    let key = search_key(query);
    if let Some(value) = ProviderCache::get(pool, PROVIDER, &key).await? {
        return Ok(value);
    }
    let value = request_json(
        client
            .get(format!("{WEB_API_BASE}/search/song"))
            .header("User-Agent", BROWSER_USER_AGENT)
            .header("Accept", "application/json")
            .query(&[("q", query)]),
    )
    .await?;
    ProviderCache::put(pool, PROVIDER, &key, &value, Utc::now() + Duration::days(7)).await?;
    Ok(value)
}

async fn song_detail(pool: &SqlitePool, client: &Client, id: i64) -> Result<Value> {
    let key = format!("song:{id}");
    if let Some(value) = ProviderCache::get(pool, PROVIDER, &key).await? {
        return Ok(value);
    }
    let value = request_json(
        client
            .get(format!("{WEB_API_BASE}/songs/{id}"))
            .header("User-Agent", BROWSER_USER_AGENT)
            .header("Accept", "application/json"),
    )
    .await?;
    ProviderCache::put(
        pool,
        PROVIDER,
        &key,
        &value,
        Utc::now() + Duration::days(14),
    )
    .await?;
    Ok(value)
}

async fn request_json(request: RequestBuilder) -> Result<Value> {
    let response = request.send().await?.error_for_status()?;
    let raw = response.json::<Value>().await?;
    if let Some(status) = raw["meta"]["status"].as_u64()
        && status != 200
    {
        let message = raw["meta"]["message"]
            .as_str()
            .unwrap_or("unknown Genius API error");
        bail!("Genius API error {status}: {message}");
    }
    Ok(raw)
}

fn search_results(raw: &Value) -> Vec<&Value> {
    raw["response"]["hits"]
        .as_array()
        .or_else(|| {
            raw["response"]["sections"]
                .as_array()?
                .iter()
                .find(|section| section["type"].as_str() == Some("song"))?["hits"]
                .as_array()
        })
        .into_iter()
        .flatten()
        .filter(|hit| hit["type"].as_str().is_none_or(|kind| kind == "song"))
        .filter_map(|hit| hit.get("result"))
        .collect()
}

fn candidate_from_song(song: &Value) -> Option<Candidate> {
    let title = nonempty(song["title"].as_str())?;
    let artist = nonempty(
        song["artist_names"]
            .as_str()
            .or_else(|| song["primary_artist"]["name"].as_str()),
    )?;
    let release_date = release_date(song);
    let album = nonempty(song["album"]["name"].as_str()).map(str::to_owned);
    Some(Candidate {
        provider: PROVIDER.into(),
        title: title.to_owned(),
        artist: artist.to_owned(),
        album_artist: nonempty(song["primary_artist"]["name"].as_str()).map(str::to_owned),
        album,
        year: release_date
            .as_deref()
            .and_then(|date| date.get(..4))
            .map(str::to_owned),
        release_date,
        genre: nonempty(song["primary_tag"]["name"].as_str()).map(str::to_owned),
        cover_url: nonempty(
            song["album"]["cover_art_url"]
                .as_str()
                .or_else(|| song["song_art_image_url"].as_str())
                .or_else(|| song["header_image_url"].as_str()),
        )
        .map(str::to_owned),
        score_breakdown: Some(serde_json::json!({"source": "genius_catalog_search"}).to_string()),
        raw_json: song.to_string(),
        ..Default::default()
    })
}

fn release_date(song: &Value) -> Option<String> {
    nonempty(song["release_date"].as_str())
        .map(str::to_owned)
        .or_else(|| {
            let parts = &song["release_date_components"];
            let year = parts["year"].as_i64()?;
            match (parts["month"].as_i64(), parts["day"].as_i64()) {
                (Some(month), Some(day)) => Some(format!("{year:04}-{month:02}-{day:02}")),
                (Some(month), None) => Some(format!("{year:04}-{month:02}")),
                _ => Some(format!("{year:04}")),
            }
        })
}

fn song_path(url: &str) -> Result<String> {
    let parsed = reqwest::Url::parse(url).context("invalid Genius URL")?;
    if parsed.scheme() != "https"
        || !matches!(parsed.host_str(), Some("genius.com" | "www.genius.com"))
    {
        bail!("only HTTPS Genius song links are supported");
    }
    let path = parsed.path().trim_end_matches('/');
    if path.matches('/').count() != 1 || !path.ends_with("-lyrics") {
        bail!("Genius URL must point to a song lyrics page");
    }
    Ok(path.to_owned())
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn song() -> Value {
        serde_json::json!({
            "id": 2471960,
            "title": "Lonely Day",
            "artist_names": "System Of A Down",
            "primary_artist": {"name": "System Of A Down"},
            "album": {
                "name": "Hypnotize",
                "cover_art_url": "https://images.genius.com/hypnotize.png"
            },
            "release_date_components": {"year": 2005, "month": 11, "day": 18},
            "song_art_image_url": "https://images.genius.com/lonely-day.jpg"
        })
    }

    #[test]
    fn parses_song_metadata_and_cover() {
        let candidate = candidate_from_song(&song()).unwrap();
        assert_eq!(candidate.provider, "genius");
        assert_eq!(candidate.title, "Lonely Day");
        assert_eq!(candidate.artist, "System Of A Down");
        assert_eq!(candidate.album.as_deref(), Some("Hypnotize"));
        assert_eq!(candidate.release_date.as_deref(), Some("2005-11-18"));
        assert_eq!(candidate.year.as_deref(), Some("2005"));
        assert_eq!(
            candidate.cover_url.as_deref(),
            Some("https://images.genius.com/hypnotize.png")
        );
    }

    #[test]
    fn parses_keyless_web_search_sections() {
        let raw = serde_json::json!({
            "response": {"sections": [
                {"type": "song", "hits": [{"type": "song", "result": song()}]}
            ]}
        });
        let results = search_results(&raw);
        assert_eq!(results.len(), 1);
        assert_eq!(results[0]["title"], "Lonely Day");
    }

    #[test]
    fn accepts_only_genius_song_pages() {
        assert_eq!(
            song_path("https://genius.com/System-of-a-down-lonely-day-lyrics").unwrap(),
            "/System-of-a-down-lonely-day-lyrics"
        );
        assert!(song_path("https://genius.com/artists/System-of-a-down").is_err());
        assert!(song_path("https://example.com/System-of-a-down-lonely-day-lyrics").is_err());
    }
}
