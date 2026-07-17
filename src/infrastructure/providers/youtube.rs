use super::Candidate;
use crate::infrastructure::provider_cache::ProviderCache;
use anyhow::{Result, bail};
use chrono::{Duration, Utc};
use reqwest::Client;
use serde_json::Value;
use sqlx::SqlitePool;

pub async fn lookup_url(client: &Client, url: &str) -> Result<Candidate> {
    let parsed = reqwest::Url::parse(url)?;
    let host = parsed.host_str().unwrap_or_default();
    if !matches!(
        host,
        "youtube.com" | "www.youtube.com" | "youtu.be" | "m.youtube.com"
    ) {
        bail!("only YouTube source links are supported");
    }
    let raw = client
        .get("https://www.youtube.com/oembed")
        .query(&[("url", url), ("format", "json")])
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    candidate_from_oembed(&raw).ok_or_else(|| anyhow::anyhow!("YouTube returned no title"))
}

fn candidate_from_oembed(raw: &Value) -> Option<Candidate> {
    let cleaned = clean_video_title(raw["title"].as_str()?);
    let (artist, title) = split_artist_title(&cleaned).unwrap_or_else(|| {
        (
            raw["author_name"]
                .as_str()
                .unwrap_or("Unknown YouTube Artist")
                .to_owned(),
            cleaned,
        )
    });
    let credits = crate::domain::credits::normalize_featured(&artist, &title);
    Some(Candidate {
        provider: "youtube".into(),
        title: credits.title,
        artist: credits.artist,
        cover_url: raw["thumbnail_url"].as_str().map(str::to_owned),
        score: 92.0,
        score_breakdown: Some(
            serde_json::json!({
                "source": "youtube_user_source_url",
                "sources": ["YouTube"],
                "user_verified_source": true
            })
            .to_string(),
        ),
        raw_json: raw.to_string(),
        ..Default::default()
    })
}

pub async fn lookup_filename_id(
    pool: &SqlitePool,
    client: &Client,
    api_key: &str,
    filename: &str,
) -> Result<Vec<Candidate>> {
    let Some(video_id) = extract_video_id(filename) else {
        return Ok(Vec::new());
    };
    let cache_key = format!("video:{video_id}");
    let raw = if let Some(value) = ProviderCache::get(pool, "youtube", &cache_key).await? {
        value
    } else {
        let value = client
            .get("https://www.googleapis.com/youtube/v3/videos")
            .query(&[
                ("part", "snippet,contentDetails"),
                ("id", video_id.as_str()),
                ("key", api_key),
            ])
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        if let Some(message) = value["error"]["message"].as_str() {
            bail!("YouTube API error: {message}");
        }
        ProviderCache::put(
            pool,
            "youtube",
            &cache_key,
            &value,
            Utc::now() + Duration::days(30),
        )
        .await?;
        value
    };
    Ok(parse_video(&raw).into_iter().collect())
}

fn extract_video_id(filename: &str) -> Option<String> {
    let stem = filename.rsplit_once('.').map_or(filename, |(stem, _)| stem);
    let tail = stem.chars().rev().take(11).collect::<String>();
    let candidate = tail.chars().rev().collect::<String>();
    (candidate.len() == 11
        && candidate
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '_' | '-')))
    .then_some(candidate)
}

fn parse_video(raw: &Value) -> Option<Candidate> {
    let video = raw["items"].as_array()?.first()?;
    let raw_title = video["snippet"]["title"].as_str()?;
    let cleaned = clean_video_title(raw_title);
    let (artist, title) = split_artist_title(&cleaned).unwrap_or_else(|| {
        let channel = video["snippet"]["channelTitle"]
            .as_str()
            .unwrap_or("Unknown YouTube Artist")
            .trim_end_matches(" - Topic")
            .trim_end_matches("VEVO")
            .trim()
            .to_owned();
        (channel, cleaned.clone())
    });
    let credits = crate::domain::credits::normalize_featured(&artist, &title);
    let published = video["snippet"]["publishedAt"].as_str();
    let cover_url = ["maxres", "standard", "high", "medium", "default"]
        .into_iter()
        .find_map(|size| video["snippet"]["thumbnails"][size]["url"].as_str())
        .map(str::to_owned);
    Some(Candidate {
        provider: "youtube".into(),
        title: credits.title,
        artist: credits.artist,
        year: published.and_then(|date| date.get(..4)).map(str::to_owned),
        release_date: published.and_then(|date| date.get(..10)).map(str::to_owned),
        cover_url,
        duration_delta: video["contentDetails"]["duration"]
            .as_str()
            .and_then(parse_iso8601_duration),
        score_breakdown: Some(
            serde_json::json!({
                "source": "youtube_exact_video_id",
                "sources": ["YouTube"],
                "supporting_evidence_only": true
            })
            .to_string(),
        ),
        raw_json: video.to_string(),
        ..Default::default()
    })
}

fn clean_video_title(value: &str) -> String {
    let mut value = value.trim().to_owned();
    let lower = value.to_ascii_lowercase();
    for marker in [
        " (official video)",
        " [official video]",
        " (official audio)",
        " [official audio]",
        " (lyrics)",
        " [lyrics]",
        " (lyric video)",
        " | official music video",
        " | official video",
        " | official audio",
    ] {
        if lower.ends_with(marker) {
            value.truncate(value.len() - marker.len());
            break;
        }
    }
    value.trim().to_owned()
}

fn split_artist_title(value: &str) -> Option<(String, String)> {
    for separator in [" - ", " – ", " — ", " | "] {
        if let Some((artist, title)) = value.split_once(separator)
            && !artist.trim().is_empty()
            && !title.trim().is_empty()
        {
            return Some((artist.trim().to_owned(), title.trim().to_owned()));
        }
    }
    None
}

fn parse_iso8601_duration(value: &str) -> Option<f64> {
    let value = value.strip_prefix("PT")?;
    let mut number = String::new();
    let mut seconds = 0_u64;
    for ch in value.chars() {
        if ch.is_ascii_digit() {
            number.push(ch);
            continue;
        }
        let amount = number.parse::<u64>().ok()?;
        number.clear();
        seconds += match ch {
            'H' => amount * 3600,
            'M' => amount * 60,
            'S' => amount,
            _ => return None,
        };
    }
    number.is_empty().then_some(seconds as f64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn extracts_video_id_from_download_filename() {
        assert_eq!(
            extract_video_id("Iraj_Tahmas_OzT_beHLfEo.mp3").as_deref(),
            Some("OzT_beHLfEo")
        );
        assert_eq!(extract_video_id("ordinary song.mp3"), None);
    }

    #[test]
    fn parses_official_video_metadata() {
        let candidate = parse_video(&serde_json::json!({"items": [{
            "snippet": {"title": "Artist - Song (Official Video)", "channelTitle": "ArtistVEVO",
                "publishedAt": "2020-04-03T00:00:00Z", "thumbnails": {"high": {"url": "cover"}}},
            "contentDetails": {"duration": "PT3M21S"}
        }]}))
        .unwrap();
        assert_eq!(candidate.artist, "Artist");
        assert_eq!(candidate.title, "Song");
        assert_eq!(candidate.duration_delta, Some(201.0));
        assert_eq!(candidate.year.as_deref(), Some("2020"));
    }

    #[test]
    fn oembed_keeps_featured_artist_in_title() {
        let candidate = candidate_from_oembed(&serde_json::json!({
            "title": "Arta - Mi Amor (feat. Saaren) | OFFICIAL MUSIC VIDEO",
            "author_name": "ARTA", "thumbnail_url": "cover"
        }))
        .unwrap();
        assert_eq!(candidate.title, "Mi Amor (feat. Saaren)");
        assert_eq!(candidate.artist, "Arta");
        assert_eq!(candidate.cover_url.as_deref(), Some("cover"));
    }
}
