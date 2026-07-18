use super::Candidate;
use crate::infrastructure::provider_cache::{ProviderCache, search_key};
use anyhow::{Context, Result, bail};
use chrono::{Duration, NaiveDate, Utc};
use reqwest::{Client, Url};
use serde_json::Value;
use sqlx::SqlitePool;

const PROVIDER: &str = "navahang";
const BASE_URL: &str = "https://www.navahang.com";
const BROWSER_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/138 Safari/537.36";

pub async fn search(
    pool: &SqlitePool,
    client: &Client,
    title: &str,
    artist: Option<&str>,
) -> Result<Vec<Candidate>> {
    let query = match artist.map(str::trim).filter(|value| !value.is_empty()) {
        Some(artist) => format!("{artist} {}", title.trim()),
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
            .get(format!("{BASE_URL}/main-search.php"))
            .header("Accept", "application/json")
            .header("User-Agent", BROWSER_USER_AGENT)
            .query(&[
                ("q", query.as_str()),
                ("size", "10"),
                ("suggestion", "true"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        ProviderCache::put(pool, PROVIDER, &key, &value, Utc::now() + Duration::days(7)).await?;
        value
    };

    let mut candidates = parse_search_results(&raw);
    if let Some(path) = raw["MP3"]
        .as_array()
        .and_then(|tracks| tracks.first())
        .and_then(|track| track["url"].as_str())
        && let Ok(url) = Url::parse(BASE_URL).and_then(|base| base.join(path))
        && let Ok(candidate) = lookup_url(pool, client, url.as_str()).await
    {
        if candidates.is_empty() {
            candidates.push(candidate);
        } else {
            candidates[0] = candidate;
        }
    }
    Ok(candidates)
}

pub async fn lookup_url(pool: &SqlitePool, client: &Client, url: &str) -> Result<Candidate> {
    let (canonical, slug) = song_url(url)?;
    let key = format!("song:{slug}");
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
        let value = page_metadata(&html, canonical.as_str())
            .ok_or_else(|| anyhow::anyhow!("Navahang returned no usable song metadata"))?;
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
    let mut candidate = candidate_from_page_metadata(&raw)
        .ok_or_else(|| anyhow::anyhow!("Navahang returned incomplete song metadata"))?;
    candidate.score = 96.0;
    candidate.score_breakdown = Some(
        serde_json::json!({
            "source": "navahang_user_source_url",
            "sources": ["Navahang"],
            "user_verified_source": true,
            "final_score": 96.0
        })
        .to_string(),
    );
    Ok(candidate)
}

fn song_url(url: &str) -> Result<(Url, String)> {
    let parsed = Url::parse(url).context("invalid Navahang URL")?;
    if parsed.scheme() != "https"
        || !matches!(parsed.host_str(), Some("navahang.com" | "www.navahang.com"))
    {
        bail!("only HTTPS Navahang song links are supported");
    }
    let segments = parsed
        .path_segments()
        .map(|parts| parts.filter(|part| !part.is_empty()).collect::<Vec<_>>())
        .unwrap_or_default();
    if segments.len() != 2 || segments[0] != "mp3" {
        bail!("Navahang URL must point to an MP3 song page");
    }
    let slug = segments[1].to_ascii_lowercase();
    let canonical = Url::parse(&format!("{BASE_URL}/mp3/{slug}/"))?;
    Ok((canonical, slug))
}

fn parse_search_results(raw: &Value) -> Vec<Candidate> {
    raw["MP3"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(candidate_from_search_track)
        .take(10)
        .collect()
}

fn candidate_from_search_track(track: &Value) -> Option<Candidate> {
    let title = nonempty(track["song_name"].as_str())?;
    let artist = nonempty(track["artist_name"].as_str())?;
    Some(Candidate {
        provider: PROVIDER.into(),
        title: title.to_owned(),
        artist: artist.to_owned(),
        album_artist: Some(artist.to_owned()),
        cover_url: nonempty(track["main_image"].as_str()).map(str::to_owned),
        score_breakdown: Some(serde_json::json!({"source":"navahang_catalog_search"}).to_string()),
        raw_json: track.to_string(),
        ..Default::default()
    })
}

fn page_metadata(html: &str, url: &str) -> Option<Value> {
    let title = data_content(html, "<h1", "text-truncate")
        .or_else(|| meta_content(html, "property", "og:title").and_then(title_from_og))?;
    let artist = data_content(html, "<h2", "text-truncate")
        .or_else(|| meta_content(html, "property", "og:title").and_then(artist_from_og))?;
    let release_date = date_added(html);
    let credits = credit_region(html);
    Some(serde_json::json!({
        "url": url,
        "title": title,
        "artist": artist,
        "cover_url": meta_content(html, "property", "og:image"),
        "duration": meta_content(html, "name", "music:duration").and_then(|value| value.parse::<f64>().ok()),
        "release_date": release_date,
        "composer": credit_field(credits, &["Music", "Composer"]),
        "label": credit_field(credits, &["Label"]),
        "lyrics_by": credit_field(credits, &["Lyrics", "Songwriter"]),
        "producer": credit_field(credits, &["Music Producer", "Producer"])
    }))
}

fn credit_region(html: &str) -> &str {
    let Some(start) = html.find("<div class=\"mp3-page") else {
        return "";
    };
    let region = &html[start..];
    let end = region.find("mp3-buttons").unwrap_or(region.len());
    &region[..end]
}

fn candidate_from_page_metadata(raw: &Value) -> Option<Candidate> {
    let title = nonempty(raw["title"].as_str())?;
    let artist = nonempty(raw["artist"].as_str())?;
    let release_date = nonempty(raw["release_date"].as_str()).map(str::to_owned);
    Some(Candidate {
        provider: PROVIDER.into(),
        title: title.to_owned(),
        artist: artist.to_owned(),
        album_artist: Some(artist.to_owned()),
        year: release_date
            .as_deref()
            .and_then(|date| date.get(..4))
            .map(str::to_owned),
        release_date,
        composer: nonempty(raw["composer"].as_str()).map(str::to_owned),
        label: nonempty(raw["label"].as_str()).map(str::to_owned),
        cover_url: nonempty(raw["cover_url"].as_str()).map(str::to_owned),
        duration_delta: raw["duration"].as_f64(),
        raw_json: raw.to_string(),
        ..Default::default()
    })
}

fn data_content(html: &str, tag_start: &str, required_class: &str) -> Option<String> {
    let mut remaining = html;
    while let Some(start) = remaining.find(tag_start) {
        remaining = &remaining[start..];
        let end = remaining.find('>')?;
        let tag = &remaining[..=end];
        if tag.contains(required_class)
            && let Some(value) = attribute(tag, "data-content")
        {
            return nonempty(Some(&value)).map(str::to_owned);
        }
        remaining = &remaining[end + 1..];
    }
    None
}

fn meta_content(html: &str, key: &str, wanted: &str) -> Option<String> {
    let mut remaining = html;
    while let Some(start) = remaining.find("<meta") {
        remaining = &remaining[start..];
        let end = remaining.find('>')?;
        let tag = &remaining[..=end];
        if attribute(tag, key).as_deref() == Some(wanted)
            && let Some(content) = attribute(tag, "content")
        {
            return nonempty(Some(&content)).map(str::to_owned);
        }
        remaining = &remaining[end + 1..];
    }
    None
}

fn attribute(tag: &str, name: &str) -> Option<String> {
    for quote in ['"', '\''] {
        let marker = format!("{name}={quote}");
        if let Some(start) = tag.find(&marker) {
            let value = &tag[start + marker.len()..];
            let end = value.find(quote)?;
            return Some(decode_html(&value[..end]));
        }
    }
    None
}

fn credit_field(html: &str, labels: &[&str]) -> Option<String> {
    let lower = html.to_ascii_lowercase();
    for label in labels {
        let marker = format!("{}:", label.to_ascii_lowercase());
        let mut offset = 0;
        while let Some(relative) = lower[offset..].find(&marker) {
            let start = offset + relative;
            let value_start = start + marker.len();
            let starts_credit_value = html[value_start..]
                .chars()
                .next()
                .is_some_and(char::is_whitespace);
            if starts_credit_value {
                let value = &html[value_start..];
                let end = value
                    .find("<br")
                    .or_else(|| value.find("</div"))
                    .unwrap_or(value.len());
                let value = strip_tags(&value[..end]);
                if !value.is_empty() {
                    return Some(value);
                }
            }
            offset = value_start;
        }
    }
    None
}

fn date_added(html: &str) -> Option<String> {
    let marker = "Date Added:";
    let start = html.find(marker)? + marker.len();
    let rest = html[start..].trim_start();
    let value = rest
        .split(|character: char| character == '<' || character.is_whitespace())
        .next()?;
    NaiveDate::parse_from_str(value, "%d/%m/%Y")
        .ok()
        .map(|date| date.format("%Y-%m-%d").to_string())
}

fn title_from_og(value: String) -> Option<String> {
    let (_, title) = value.split_once(" - ")?;
    nonempty(Some(title.trim_matches(['\'', '"']))).map(str::to_owned)
}

fn artist_from_og(value: String) -> Option<String> {
    let (artist, _) = value.split_once(" - ")?;
    nonempty(Some(artist)).map(str::to_owned)
}

fn strip_tags(value: &str) -> String {
    let mut text = String::new();
    let mut in_tag = false;
    for character in value.chars() {
        match character {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => text.push(character),
            _ => {}
        }
    }
    decode_html(&text).trim().to_owned()
}

fn decode_html(value: &str) -> String {
    value
        .replace("&amp;", "&")
        .replace("&#038;", "&")
        .replace("&#38;", "&")
        .replace("&quot;", "\"")
        .replace("&#039;", "'")
        .replace("&#39;", "'")
        .replace("&#8211;", "–")
        .replace("&#8217;", "’")
        .replace("&nbsp;", " ")
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_exact_search_result() {
        let candidates = parse_search_results(&serde_json::json!({"MP3": [{
            "id": "261734",
            "url": "/mp3/hoomaan-darling/",
            "main_image": "https://media.navahang.me/2024/05/Hoomaan-Darling.jpg",
            "song_name": "Darling",
            "artist_name": "Hoomaan"
        }]}));
        let candidate = &candidates[0];
        assert_eq!(candidate.provider, "navahang");
        assert_eq!(candidate.title, "Darling");
        assert_eq!(candidate.artist, "Hoomaan");
        assert_eq!(candidate.album_artist.as_deref(), Some("Hoomaan"));
        assert!(
            candidate
                .cover_url
                .as_deref()
                .unwrap()
                .ends_with("Hoomaan-Darling.jpg")
        );
    }

    #[test]
    fn parses_page_metadata_duration_date_and_credits() {
        let html = r#"
            <meta property="og:title" content="Hoomaan - 'Darling'">
            <meta property="og:image" content="https://www.navahang.com/images/Hoomaan-Darling.jpg">
            <meta name="music:duration" content="237">
            <div class="mp3-page position-relative songplayerbg">
            <h1 class="text-truncate" data-content="Darling">Darling</h1>
            <h2 class="text-truncate mb-0" data-content="Hoomaan">Hoomaan</h2>
            Lyrics: Writer<br />Music: Arman Miladi<br />Label: Navahang Records<br />
            <div class="col-12 col-md-3 mp3-buttons">
            <i class="fas fa-calendar-alt"></i> Date Added: 05/12/2017
            <script>data = {lyrics: lyricsText}</script>
        "#;
        let raw = page_metadata(html, "https://www.navahang.com/mp3/hoomaan-darling/").unwrap();
        let candidate = candidate_from_page_metadata(&raw).unwrap();
        assert_eq!(candidate.title, "Darling");
        assert_eq!(candidate.artist, "Hoomaan");
        assert_eq!(candidate.duration_delta, Some(237.0));
        assert_eq!(candidate.release_date.as_deref(), Some("2017-12-05"));
        assert_eq!(candidate.year.as_deref(), Some("2017"));
        assert_eq!(candidate.composer.as_deref(), Some("Arman Miladi"));
        assert_eq!(candidate.label.as_deref(), Some("Navahang Records"));
        assert_eq!(raw["lyrics_by"].as_str(), Some("Writer"));
    }

    #[test]
    fn accepts_only_navahang_mp3_pages() {
        let (url, slug) =
            song_url("https://www.navahang.com/mp3/hoomaan-darling/?group=1").unwrap();
        assert_eq!(
            url.as_str(),
            "https://www.navahang.com/mp3/hoomaan-darling/"
        );
        assert_eq!(slug, "hoomaan-darling");
        assert!(song_url("https://www.navahang.com/artist/hoomaan/").is_err());
        assert!(song_url("https://example.com/mp3/hoomaan-darling/").is_err());
    }

    #[tokio::test]
    #[ignore = "live Navahang page probe"]
    async fn live_exact_example() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("navahang.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let candidate = lookup_url(
            &pool,
            &Client::new(),
            "https://www.navahang.com/mp3/hoomaan-darling/",
        )
        .await
        .unwrap();
        assert_eq!(candidate.title, "Darling");
        assert_eq!(candidate.artist, "Hoomaan");
        assert_eq!(candidate.duration_delta, Some(237.0));
        assert_eq!(candidate.release_date.as_deref(), Some("2017-12-05"));
        let cover_url = candidate.cover_url.as_deref().unwrap();
        assert!(cover_url.ends_with(".jpg"));
        let bytes =
            crate::infrastructure::providers::cover_art_archive::fetch(&Client::new(), cover_url)
                .await
                .unwrap();
        crate::infrastructure::media::tag_writer::validate_artwork(&bytes).unwrap();

        let candidates = search(&pool, &Client::new(), "Darling", Some("Hoomaan"))
            .await
            .unwrap();
        assert_eq!(candidates[0].title, "Darling");
        assert_eq!(candidates[0].artist, "Hoomaan");
        assert_eq!(candidates[0].duration_delta, Some(237.0));
    }
}
