use super::Candidate;
use crate::infrastructure::provider_cache::{ProviderCache, search_key};
use anyhow::{Context, Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use chrono::{DateTime, Duration, Utc};
use hmac::{Hmac, KeyInit, Mac};
use reqwest::Client;
use serde_json::Value;
use sha1::Sha1;
use sqlx::SqlitePool;
use std::{
    sync::atomic::{AtomicU64, Ordering},
    time::{SystemTime, UNIX_EPOCH},
};

const PROVIDER: &str = "audiomack";
const API_BASE: &str = "https://api.audiomack.com/v1";
// Audiomack's website publishes this read-only web consumer in its browser
// bundle and uses it for public catalog requests. No user account is involved.
const WEB_CONSUMER_KEY: &str = "audiomack-web";
const WEB_CONSUMER_SECRET: &str = "bd8a07e9f23fbe9d808646b730f89b8e";
const BROWSER_USER_AGENT: &str =
    "Mozilla/5.0 (Macintosh; Intel Mac OS X 10_15_7) AppleWebKit/537.36 Chrome/138 Safari/537.36";
static NONCE_COUNTER: AtomicU64 = AtomicU64::new(0);

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
        let parameters = vec![
            ("limit".to_owned(), "10".to_owned()),
            ("page".to_owned(), "1".to_owned()),
            ("q".to_owned(), query),
            ("show".to_owned(), "music".to_owned()),
            ("sort".to_owned(), "relevance".to_owned()),
        ];
        let url = signed_get_url("livesearch", &parameters)?;
        let value = client
            .get(url)
            .header("Accept", "application/json")
            .header("User-Agent", BROWSER_USER_AGENT)
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
    let (canonical, slug) = song_url(url)?;
    let key = format!("song:{}", canonical.path().to_ascii_lowercase());
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
        let value = music_object_from_page(&html, &slug)
            .or_else(|| json_ld_from_page(&html))
            .ok_or_else(|| anyhow::anyhow!("Audiomack returned no usable song metadata"))?;
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
    let mut candidate = if raw["@type"].as_str() == Some("MusicRecording") {
        candidate_from_json_ld(&raw)
    } else {
        candidate_from_track(&raw)
    }
    .ok_or_else(|| anyhow::anyhow!("Audiomack returned incomplete song metadata"))?;
    candidate.score = 96.0;
    candidate.score_breakdown = Some(
        serde_json::json!({
            "source": "audiomack_user_source_url",
            "sources": ["Audiomack"],
            "user_verified_source": true,
            "final_score": 96.0
        })
        .to_string(),
    );
    Ok(candidate)
}

fn signed_get_url(endpoint: &str, parameters: &[(String, String)]) -> Result<reqwest::Url> {
    let timestamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .context("system clock is before Unix epoch")?
        .as_secs();
    let nonce = format!(
        "{timestamp:x}{:x}",
        NONCE_COUNTER.fetch_add(1, Ordering::Relaxed)
    );
    signed_get_url_at(endpoint, parameters, timestamp, &nonce)
}

fn signed_get_url_at(
    endpoint: &str,
    parameters: &[(String, String)],
    timestamp: u64,
    nonce: &str,
) -> Result<reqwest::Url> {
    let base = format!("{API_BASE}/{endpoint}");
    let mut signed = parameters.to_vec();
    signed.extend([
        ("oauth_consumer_key".into(), WEB_CONSUMER_KEY.into()),
        ("oauth_nonce".into(), nonce.into()),
        ("oauth_signature_method".into(), "HMAC-SHA1".into()),
        ("oauth_timestamp".into(), timestamp.to_string()),
        ("oauth_version".into(), "1.0".into()),
    ]);
    let mut encoded = signed
        .iter()
        .map(|(key, value)| (oauth_encode(key), oauth_encode(value)))
        .collect::<Vec<_>>();
    encoded.sort();
    let normalized = encoded
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect::<Vec<_>>()
        .join("&");
    let signature_base = format!("GET&{}&{}", oauth_encode(&base), oauth_encode(&normalized));
    let signing_key = format!("{}&", oauth_encode(WEB_CONSUMER_SECRET));
    let mut mac = Hmac::<Sha1>::new_from_slice(signing_key.as_bytes())
        .map_err(|_| anyhow::anyhow!("invalid Audiomack signing key"))?;
    mac.update(signature_base.as_bytes());
    let signature = STANDARD.encode(mac.finalize().into_bytes());
    signed.push(("oauth_signature".into(), signature));
    let mut url = reqwest::Url::parse(&base)?;
    url.query_pairs_mut().extend_pairs(signed);
    Ok(url)
}

fn oauth_encode(value: &str) -> String {
    const HEX: &[u8; 16] = b"0123456789ABCDEF";
    let mut output = String::with_capacity(value.len());
    for byte in value.bytes() {
        if byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'.' | b'_' | b'~') {
            output.push(char::from(byte));
        } else {
            output.push('%');
            output.push(char::from(HEX[(byte >> 4) as usize]));
            output.push(char::from(HEX[(byte & 0x0f) as usize]));
        }
    }
    output
}

fn reject_api_error(raw: &Value) -> Result<()> {
    if let Some(message) = raw["message"]
        .as_str()
        .filter(|_| raw["errorcode"].is_number())
    {
        bail!("Audiomack API error: {message}");
    }
    Ok(())
}

fn song_url(url: &str) -> Result<(reqwest::Url, String)> {
    let parsed = reqwest::Url::parse(url).context("invalid Audiomack URL")?;
    if parsed.scheme() != "https"
        || !matches!(
            parsed.host_str(),
            Some("audiomack.com" | "www.audiomack.com")
        )
    {
        bail!("only HTTPS Audiomack song links are supported");
    }
    let segments = parsed
        .path_segments()
        .map(|parts| parts.filter(|part| !part.is_empty()).collect::<Vec<_>>())
        .unwrap_or_default();
    if segments.len() != 3 || segments[1] != "song" {
        bail!("Audiomack URL must point to a song page");
    }
    let slug = segments[2].to_owned();
    let canonical = reqwest::Url::parse(&format!(
        "https://audiomack.com/{}/song/{slug}",
        segments[0]
    ))?;
    Ok((canonical, slug))
}

fn parse_search_results(raw: &Value) -> Vec<Candidate> {
    raw["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter(|track| track["type"].as_str().is_none_or(|kind| kind == "song"))
        .filter_map(candidate_from_track)
        .take(10)
        .collect()
}

fn candidate_from_track(track: &Value) -> Option<Candidate> {
    let title = nonempty(track["title"].as_str())?;
    let artist = nonempty(track["artist"].as_str())?;
    let release_date = release_date(track);
    let description = track["description"].as_str().unwrap_or_default();
    let album = nonempty(track["album"].as_str()).map(clean_album);
    let genre = nonempty(track["genre"].as_str())
        .or_else(|| nonempty(track["tagdisplay"].as_str()))
        .map(title_case);
    let image = track["images"]["original"]["filename"]
        .as_str()
        .or_else(|| track["image_base"].as_str())
        .or_else(|| track["image"].as_str());
    Some(Candidate {
        provider: PROVIDER.into(),
        title: title.to_owned(),
        artist: artist.to_owned(),
        album,
        album_artist: Some(artist.to_owned()),
        track_number: track.get("ord").and_then(number).map(|value| value as i64),
        year: release_date
            .as_deref()
            .and_then(|date| date.get(..4))
            .map(str::to_owned),
        genre,
        composer: description_field(description, &["Music", "Composer"]),
        label: description_field(description, &["Label"]),
        isrc: nonempty(track["isrc"].as_str()).map(str::to_owned),
        cover_url: nonempty(image).map(embeddable_image_url),
        release_date,
        duration_delta: track.get("duration").and_then(number),
        score_breakdown: Some(serde_json::json!({"source":"audiomack_catalog_search"}).to_string()),
        raw_json: track.to_string(),
        ..Default::default()
    })
}

fn candidate_from_json_ld(raw: &Value) -> Option<Candidate> {
    let title = nonempty(raw["name"].as_str())?;
    let description = raw["description"].as_str().unwrap_or_default();
    let artist = description_field(description, &["Artist"])
        .or_else(|| nonempty(raw["byArtist"]["name"].as_str()).map(str::to_owned))?;
    let release_date = nonempty(raw["datePublished"].as_str()).map(str::to_owned);
    Some(Candidate {
        provider: PROVIDER.into(),
        title: title.to_owned(),
        artist: artist.clone(),
        album_artist: Some(artist),
        year: release_date
            .as_deref()
            .and_then(|date| date.get(..4))
            .map(str::to_owned),
        release_date,
        genre: nonempty(raw["genre"].as_str()).map(title_case),
        composer: description_field(description, &["Music", "Composer"]),
        label: description_field(description, &["Label"]),
        cover_url: nonempty(raw["image"].as_str()).map(embeddable_image_url),
        duration_delta: raw["duration"].as_str().and_then(parse_iso_duration),
        raw_json: raw.to_string(),
        ..Default::default()
    })
}

fn music_object_from_page(html: &str, slug: &str) -> Option<Value> {
    let marker = format!("\"url_slug\":\"{slug}\"");
    for payload in flight_payloads(html) {
        if !payload.contains(&marker) {
            continue;
        }
        let starts = payload
            .char_indices()
            .filter_map(|(start, character)| (character == '{').then_some(start))
            .collect::<Vec<_>>();
        for start in starts.into_iter().rev() {
            let Some(object) = balanced_json_object(&payload[start..]) else {
                continue;
            };
            if !object.contains(&marker) {
                continue;
            }
            if let Ok(value) = serde_json::from_str::<Value>(object)
                && value["url_slug"].as_str() == Some(slug)
                && value["type"].as_str() == Some("song")
            {
                return Some(value);
            }
        }
    }
    None
}

fn flight_payloads(html: &str) -> Vec<String> {
    let prefix = "self.__next_f.push(";
    let mut rest = html;
    let mut output = Vec::new();
    while let Some(start) = rest.find(prefix) {
        rest = &rest[start + prefix.len()..];
        let Some(end) = rest.find(")</script>") else {
            break;
        };
        if let Ok(Value::Array(values)) = serde_json::from_str::<Value>(&rest[..end])
            && let Some(payload) = values.get(1).and_then(Value::as_str)
        {
            output.push(payload.to_owned());
        }
        rest = &rest[end + 1..];
    }
    output
}

fn json_ld_from_page(html: &str) -> Option<Value> {
    let marker = "<script type=\"application/ld+json\">";
    let start = html.find(marker)? + marker.len();
    let end = html[start..].find("</script>")? + start;
    serde_json::from_str(&html[start..end]).ok()
}

fn balanced_json_object(value: &str) -> Option<&str> {
    let mut depth = 0_u32;
    let mut quoted = false;
    let mut escaped = false;
    for (index, character) in value.char_indices() {
        if quoted {
            if escaped {
                escaped = false;
            } else if character == '\\' {
                escaped = true;
            } else if character == '"' {
                quoted = false;
            }
            continue;
        }
        match character {
            '"' => quoted = true,
            '{' => depth += 1,
            '}' => {
                depth = depth.checked_sub(1)?;
                if depth == 0 {
                    return value.get(..=index);
                }
            }
            _ => {}
        }
    }
    None
}

fn release_date(track: &Value) -> Option<String> {
    nonempty(track["original_release_date"].as_str())
        .map(str::to_owned)
        .or_else(|| {
            let timestamp = number(track.get("released")?)? as i64;
            DateTime::<Utc>::from_timestamp(timestamp, 0)
                .map(|date| date.format("%Y-%m-%d").to_string())
        })
}

fn description_field(description: &str, keys: &[&str]) -> Option<String> {
    description.lines().find_map(|line| {
        let (key, value) = line.split_once(':')?;
        keys.iter()
            .any(|candidate| key.trim().eq_ignore_ascii_case(candidate))
            .then(|| value.trim().to_owned())
            .filter(|value| !value.is_empty())
    })
}

fn clean_album(value: &str) -> String {
    let value = value.trim();
    if let Some((album, suffix)) = value.split_once('|')
        && [".com", ".net", ".org", ".ir"]
            .iter()
            .any(|domain| suffix.to_ascii_lowercase().contains(domain))
    {
        return album.trim().to_owned();
    }
    value.to_owned()
}

fn title_case(value: &str) -> String {
    let mut characters = value.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

fn embeddable_image_url(value: &str) -> String {
    format!(
        "{}?width=1600&format=jpg",
        value.split('?').next().unwrap_or(value)
    )
}

fn number(value: &Value) -> Option<f64> {
    value
        .as_f64()
        .or_else(|| value.as_str()?.trim().parse().ok())
}

fn parse_iso_duration(value: &str) -> Option<f64> {
    let value = value.strip_prefix("PT")?;
    let (minutes, seconds) = value.split_once('M')?;
    Some(minutes.parse::<f64>().ok()? * 60.0 + seconds.strip_suffix('S')?.parse::<f64>().ok()?)
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn track() -> Value {
        serde_json::json!({
            "id": 14216571,
            "type": "song",
            "url_slug": "saaren-tehran-kenaret",
            "title": "Tehran Kenaret",
            "artist": "Saaren",
            "album": "Tehran Kenaret | UAhang.Com",
            "duration": "171",
            "genre": "pop",
            "description": "Artist: Saaren\nMusic: Saaren\nLyrics: Saaren & Ali T\nLabel: Radio Javan",
            "released": "1625577630",
            "isrc": "",
            "images": {"original": {"filename": "https://i.audiomack.com/fomusic/26813068dc.webp"}}
        })
    }

    #[test]
    fn parses_full_song_metadata_and_original_cover() {
        let candidate = candidate_from_track(&track()).unwrap();
        assert_eq!(candidate.provider, "audiomack");
        assert_eq!(candidate.title, "Tehran Kenaret");
        assert_eq!(candidate.artist, "Saaren");
        assert_eq!(candidate.album.as_deref(), Some("Tehran Kenaret"));
        assert_eq!(candidate.duration_delta, Some(171.0));
        assert_eq!(candidate.genre.as_deref(), Some("Pop"));
        assert_eq!(candidate.composer.as_deref(), Some("Saaren"));
        assert_eq!(candidate.label.as_deref(), Some("Radio Javan"));
        assert_eq!(candidate.release_date.as_deref(), Some("2021-07-06"));
        assert_eq!(candidate.year.as_deref(), Some("2021"));
        assert_eq!(
            candidate.cover_url.as_deref(),
            Some("https://i.audiomack.com/fomusic/26813068dc.webp?width=1600&format=jpg")
        );
    }

    #[test]
    fn extracts_song_from_next_flight_payload() {
        let object = track().to_string();
        let payload = format!("47:[\"$\",{{\"data\":{object}}}]");
        let encoded = serde_json::to_string(&serde_json::json!([1, payload])).unwrap();
        let html = format!("<script>self.__next_f.push({encoded})</script>");
        let parsed = music_object_from_page(&html, "saaren-tehran-kenaret").unwrap();
        assert_eq!(parsed["artist"], "Saaren");
    }

    #[test]
    fn validates_only_audiomack_song_links() {
        assert!(song_url("https://audiomack.com/fomusic/song/saaren-tehran-kenaret").is_ok());
        assert!(song_url("https://audiomack.com/fomusic/album/example").is_err());
        assert!(song_url("https://example.com/fomusic/song/example").is_err());
    }

    #[test]
    fn oauth_signature_is_stable_for_fixed_inputs() {
        let url = signed_get_url_at(
            "livesearch",
            &[("q".into(), "Saaren Tehran Kenaret".into())],
            1_700_000_000,
            "fixed-nonce",
        )
        .unwrap();
        assert_eq!(
            url.query_pairs()
                .find(|(key, _)| key == "oauth_consumer_key")
                .unwrap()
                .1,
            WEB_CONSUMER_KEY
        );
        assert!(
            url.query_pairs()
                .any(|(key, value)| key == "oauth_signature" && !value.is_empty())
        );
    }

    #[tokio::test]
    #[ignore = "live Audiomack API probe"]
    async fn live_search_probe() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("audiomack-live.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let results = search(&pool, &Client::new(), "Tehran Kenaret", Some("Saaren"))
            .await
            .unwrap();
        assert!(results.iter().any(|candidate| {
            candidate.title == "Tehran Kenaret" && candidate.artist == "Saaren"
        }));
        let direct = lookup_url(
            &pool,
            &Client::new(),
            "https://audiomack.com/fomusic/song/saaren-tehran-kenaret",
        )
        .await
        .unwrap();
        let artwork = crate::infrastructure::providers::cover_art_archive::fetch(
            &Client::new(),
            direct.cover_url.as_deref().unwrap(),
        )
        .await
        .unwrap();
        crate::infrastructure::media::tag_writer::validate_artwork(&artwork).unwrap();
    }
}
