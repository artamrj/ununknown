use super::Candidate;
use crate::infrastructure::provider_cache::ProviderCache;
use anyhow::{Context, Result, bail};
use chrono::{Duration, Utc};
use reqwest::{Client, Url};
use serde_json::Value;
use sqlx::SqlitePool;

const PROVIDER: &str = "shazam";
const BASE_URL: &str = "https://www.shazam.com";
const BROWSER_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/138 Safari/537.36";

pub async fn lookup_url(pool: &SqlitePool, client: &Client, url: &str) -> Result<Candidate> {
    let (canonical, song_id) = song_url(url)?;
    let key = format!("song:{song_id}");
    let raw = if let Some(value) = ProviderCache::get(pool, PROVIDER, &key).await? {
        value
    } else {
        let html = client
            .get(canonical.clone())
            .header("Accept", "text/html,application/xhtml+xml")
            .header("User-Agent", BROWSER_USER_AGENT)
            .send()
            .await?
            .error_for_status()?
            .text()
            .await?;
        let value = page_metadata(&html, &song_id, canonical.as_str())
            .ok_or_else(|| anyhow::anyhow!("Shazam returned no usable song metadata"))?;
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
    candidate_from_metadata(&raw)
        .ok_or_else(|| anyhow::anyhow!("Shazam returned incomplete song metadata"))
}

fn song_url(url: &str) -> Result<(Url, String)> {
    let parsed = Url::parse(url).context("invalid Shazam URL")?;
    if parsed.scheme() != "https"
        || !matches!(parsed.host_str(), Some("shazam.com" | "www.shazam.com"))
    {
        bail!("only HTTPS Shazam song links are supported");
    }
    let segments = parsed
        .path_segments()
        .map(|parts| parts.filter(|part| !part.is_empty()).collect::<Vec<_>>())
        .unwrap_or_default();
    let song_position = segments
        .iter()
        .position(|segment| *segment == "song")
        .filter(|position| *position <= 1)
        .ok_or_else(|| anyhow::anyhow!("Shazam URL must point to a song page"))?;
    let song_id = segments
        .get(song_position + 1)
        .filter(|value| value.chars().all(|character| character.is_ascii_digit()))
        .ok_or_else(|| anyhow::anyhow!("Shazam song link has no valid song ID"))?
        .to_string();
    let slug = segments.get(song_position + 2).copied().unwrap_or("track");
    let canonical = Url::parse(&format!("{BASE_URL}/song/{song_id}/{slug}"))?;
    Ok((canonical, song_id))
}

fn page_metadata(html: &str, song_id: &str, url: &str) -> Option<Value> {
    let attributes_marker = "\\\"attributes\\\":{\\\"type\\\":\\\"MUSIC\\\"";
    let start = html.find(attributes_marker)?;
    let payload = &html[start..];
    let track_end = payload
        .find("\\\"relationships\\\":")
        .unwrap_or(payload.len());
    let track = &payload[..track_end];
    let album_start = payload.find("\\\"album\\\":{\\\"id\\\"")?;
    let album_payload = &payload[album_start..];
    let album_end = album_payload
        .find("\\\"audioAnalysis\\\":")
        .unwrap_or(album_payload.len());
    let album = &album_payload[..album_end];

    let title = escaped_string(track, "title")?;
    let artist = escaped_string(track, "artist")?;
    let release_date = escaped_string(album, "releaseDate");
    let cover_url =
        escaped_string(track, "coverArtHq").or_else(|| escaped_string(track, "coverArt"));
    let album_name = escaped_string(album, "name");
    let album_artist =
        escaped_string(album, "albumArtistName").or_else(|| escaped_string(album, "artistName"));
    let is_single = escaped_bool(album, "isSingle").unwrap_or(false);
    let is_compilation = escaped_bool(album, "isCompilation").unwrap_or(false);
    let release_type = if is_compilation {
        Some("compilation")
    } else if is_single {
        Some("single")
    } else if album_name.is_some() {
        Some("album")
    } else {
        None
    };

    Some(serde_json::json!({
        "url": url,
        "song_id": song_id,
        "title": title,
        "artist": artist,
        "album": album_name,
        "album_artist": album_artist,
        "track_number": escaped_i64(album, "trackNumber"),
        "track_total": escaped_i64(album, "trackCount"),
        "disc_number": escaped_i64(album, "discNumber"),
        "release_date": release_date,
        "release_type": release_type,
        "is_compilation": is_compilation,
        "genre": escaped_string(track, "primary"),
        "composer": escaped_string(album, "composerName"),
        "label": escaped_string(track, "label").or_else(|| escaped_string(album, "recordLabel")),
        "isrc": escaped_string(track, "isrc").or_else(|| escaped_string(album, "isrc")),
        "cover_url": cover_url,
        "duration_seconds": escaped_i64(album, "durationInMillis").map(|value| value as f64 / 1000.0)
    }))
}

fn candidate_from_metadata(raw: &Value) -> Option<Candidate> {
    let release_date = nonempty(raw["release_date"].as_str()).map(str::to_owned);
    Some(Candidate {
        provider: PROVIDER.into(),
        title: nonempty(raw["title"].as_str())?.to_owned(),
        artist: nonempty(raw["artist"].as_str())?.to_owned(),
        album: nonempty(raw["album"].as_str()).map(str::to_owned),
        album_artist: nonempty(raw["album_artist"].as_str()).map(str::to_owned),
        track_number: raw["track_number"].as_i64(),
        track_total: raw["track_total"].as_i64(),
        disc_number: raw["disc_number"].as_i64(),
        year: release_date
            .as_deref()
            .and_then(|date| date.get(..4))
            .map(str::to_owned),
        genre: nonempty(raw["genre"].as_str()).map(str::to_owned),
        composer: nonempty(raw["composer"].as_str()).map(str::to_owned),
        label: nonempty(raw["label"].as_str()).map(str::to_owned),
        isrc: nonempty(raw["isrc"].as_str()).map(str::to_owned),
        cover_url: nonempty(raw["cover_url"].as_str()).map(str::to_owned),
        recording_id: nonempty(raw["song_id"].as_str()).map(str::to_owned),
        release_date,
        release_type: nonempty(raw["release_type"].as_str()).map(str::to_owned),
        is_compilation: raw["is_compilation"].as_bool().unwrap_or(false),
        duration_delta: raw["duration_seconds"].as_f64(),
        score: 97.0,
        score_breakdown: Some(
            serde_json::json!({
                "source": "shazam_user_source_url",
                "sources": ["Shazam"],
                "user_verified_source": true,
                "final_score": 97.0
            })
            .to_string(),
        ),
        raw_json: raw.to_string(),
        ..Default::default()
    })
}

fn escaped_string(region: &str, key: &str) -> Option<String> {
    let marker = format!("\\\"{key}\\\":\\\"");
    let rest = region.get(region.find(&marker)? + marker.len()..)?;
    let end = rest.find("\\\"")?;
    serde_json::from_str::<String>(&format!("\"{}\"", &rest[..end]))
        .ok()
        .map(|value| decode_html_entities(&value))
        .filter(|value| !value.trim().is_empty())
}

fn escaped_i64(region: &str, key: &str) -> Option<i64> {
    escaped_primitive(region, key)?.parse().ok()
}

fn escaped_bool(region: &str, key: &str) -> Option<bool> {
    escaped_primitive(region, key)?.parse().ok()
}

fn escaped_primitive<'a>(region: &'a str, key: &str) -> Option<&'a str> {
    let marker = format!("\\\"{key}\\\":");
    let rest = region.get(region.find(&marker)? + marker.len()..)?;
    let end = rest.find([',', '}']).unwrap_or(rest.len());
    Some(rest[..end].trim())
}

fn decode_html_entities(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&quot;", "\"")
        .replace("&#x27;", "'")
        .replace("&#39;", "'")
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn accepts_localized_and_unlocalized_shazam_song_links() {
        let (_, id) = song_url(
            "https://www.shazam.com/en-us/song/1505820972/eyes-on-you#referrer=shazamformac",
        )
        .unwrap();
        assert_eq!(id, "1505820972");
        assert!(song_url("https://www.shazam.com/song/1505820972/eyes-on-you").is_ok());
        assert!(song_url("https://www.shazam.com/artist/twenty7/1505810058").is_err());
    }

    #[test]
    fn parses_embedded_shazam_and_apple_catalog_fields() {
        let html = r#"before \"attributes\":{\"type\":\"MUSIC\",\"title\":\"Eyes on You\",\"artist\":\"Twenty7\",\"label\":\"ONLY THE HIGHEST\",\"isrc\":\"QZDA52082376\",\"images\":{\"coverArtHq\":\"https://example.test/cover.jpg\"},\"genres\":{\"primary\":\"Alternative\"},\"relationships\":{}},\"album\":{\"id\":\"1505820971\",\"attributes\":{\"artistName\":\"Twenty7\",\"isCompilation\":false,\"isSingle\":true,\"name\":\"Eyes on You - Single\",\"recordLabel\":\"ONLY THE HIGHEST\",\"releaseDate\":\"2020-04-02\",\"trackCount\":1},\"tracks\":{\"data\":[{\"attributes\":{\"albumArtistName\":\"Twenty7\",\"composerName\":\"Ash Sharma\",\"discNumber\":1,\"durationInMillis\":148000,\"trackNumber\":1}}]}},\"audioAnalysis\":{} after"#;
        let raw = page_metadata(
            html,
            "1505820972",
            "https://www.shazam.com/song/1505820972/x",
        )
        .unwrap();
        let candidate = candidate_from_metadata(&raw).unwrap();
        assert_eq!(candidate.title, "Eyes on You");
        assert_eq!(candidate.artist, "Twenty7");
        assert_eq!(candidate.album.as_deref(), Some("Eyes on You - Single"));
        assert_eq!(candidate.isrc.as_deref(), Some("QZDA52082376"));
        assert_eq!(candidate.composer.as_deref(), Some("Ash Sharma"));
        assert_eq!(candidate.release_date.as_deref(), Some("2020-04-02"));
        assert_eq!(candidate.duration_delta, Some(148.0));
        assert_eq!(candidate.track_number, Some(1));
        assert_eq!(candidate.track_total, Some(1));
        assert_eq!(candidate.release_type.as_deref(), Some("single"));
    }

    #[tokio::test]
    #[ignore = "live Shazam page probe"]
    async fn resolves_exact_shazam_link_with_isrc_and_cover() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("shazam.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let candidate = lookup_url(
            &pool,
            &Client::new(),
            "https://www.shazam.com/song/1505820972/eyes-on-you#referrer=shazamformac",
        )
        .await
        .unwrap();
        assert_eq!(candidate.title, "Eyes on You");
        assert_eq!(candidate.artist, "Twenty7");
        assert_eq!(candidate.isrc.as_deref(), Some("QZDA52082376"));
        assert!(candidate.cover_url.is_some());
    }
}
