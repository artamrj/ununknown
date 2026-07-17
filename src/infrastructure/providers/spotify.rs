use super::Candidate;
use anyhow::{Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::Client;
use serde_json::Value;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Default)]
pub struct SpotifyAuth {
    token: Mutex<Option<CachedToken>>,
}

struct CachedToken {
    value: String,
    expires_at: Instant,
}

pub async fn search(
    client: &Client,
    auth: &SpotifyAuth,
    client_id: &str,
    client_secret: &str,
    title: &str,
    artist: Option<&str>,
    isrcs: &[String],
) -> Result<Vec<Candidate>> {
    let token = access_token(client, auth, client_id, client_secret).await?;
    let mut queries = isrcs
        .iter()
        .filter(|isrc| !isrc.trim().is_empty())
        .map(|isrc| format!("isrc:{}", isrc.trim()))
        .collect::<Vec<_>>();
    if queries.is_empty() {
        queries.push(match artist.filter(|value| !value.trim().is_empty()) {
            Some(artist) => format!("track:\"{title}\" artist:\"{artist}\""),
            None => format!("track:\"{title}\""),
        });
    }
    queries.sort();
    queries.dedup();
    let mut candidates = Vec::new();
    for query in queries.into_iter().take(3) {
        let raw = client
            .get("https://api.spotify.com/v1/search")
            .bearer_auth(&token)
            .query(&[("q", query.as_str()), ("type", "track"), ("limit", "10")])
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        candidates.extend(parse_results(&raw));
    }
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|candidate| {
        candidate
            .raw_json
            .parse::<Value>()
            .ok()
            .and_then(|value| value["id"].as_str().map(str::to_owned))
            .is_none_or(|id| seen.insert(id))
    });
    Ok(candidates)
}

pub async fn lookup_url(client: &Client, url: &str) -> Result<Candidate> {
    let parsed = reqwest::Url::parse(url)?;
    if parsed.scheme() != "https"
        || parsed.host_str() != Some("open.spotify.com")
        || !parsed.path().starts_with("/track/")
    {
        bail!("only HTTPS Spotify track links are supported");
    }
    let mut endpoint = reqwest::Url::parse("https://open.spotify.com/oembed")?;
    endpoint.query_pairs_mut().append_pair("url", url);
    let raw = crate::infrastructure::resilient_http::get(client, endpoint.as_str())
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    let title = raw["title"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Spotify returned no track title"))?;
    let cover_url = raw["thumbnail_url"]
        .as_str()
        .map(upgrade_artwork_url)
        .ok_or_else(|| anyhow::anyhow!("Spotify returned no cover artwork"))?;
    Ok(Candidate {
        provider: "spotify".into(),
        title: title.to_owned(),
        // Spotify oEmbed deliberately omits performers. The manual editor keeps
        // its existing artist when this field is empty.
        artist: String::new(),
        cover_url: Some(cover_url),
        score: 94.0,
        score_breakdown: Some(
            serde_json::json!({
                "source": "spotify_user_source_url",
                "sources": ["Spotify"],
                "user_verified_source": true,
                "artwork_only_artist_preserved": true
            })
            .to_string(),
        ),
        raw_json: raw.to_string(),
        ..Default::default()
    })
}

pub fn upgrade_artwork_url(url: &str) -> String {
    url.replace("ab67616d00001e02", "ab67616d0000b273")
        .replace("ab67616d00004851", "ab67616d0000b273")
}

async fn access_token(
    client: &Client,
    auth: &SpotifyAuth,
    client_id: &str,
    client_secret: &str,
) -> Result<String> {
    let mut cached = auth.token.lock().await;
    if let Some(token) = cached.as_ref()
        && token.expires_at > Instant::now() + Duration::from_secs(30)
    {
        return Ok(token.value.clone());
    }
    let authorization = STANDARD.encode(format!("{client_id}:{client_secret}"));
    let raw = client
        .post("https://accounts.spotify.com/api/token")
        .header("Authorization", format!("Basic {authorization}"))
        .form(&[("grant_type", "client_credentials")])
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    let value = raw["access_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("Spotify token response had no access token"))?
        .to_owned();
    let lifetime = raw["expires_in"].as_u64().unwrap_or(3600).max(60);
    *cached = Some(CachedToken {
        value: value.clone(),
        expires_at: Instant::now() + Duration::from_secs(lifetime),
    });
    Ok(value)
}

fn parse_results(raw: &Value) -> Vec<Candidate> {
    raw["tracks"]["items"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|track| {
            if track["type"].as_str().is_some_and(|value| value != "track") {
                return None;
            }
            let release_date = track["album"]["release_date"].as_str().map(str::to_owned);
            let album_type = track["album"]["album_type"].as_str().map(str::to_owned);
            Some(Candidate {
                provider: "spotify".into(),
                title: track["name"].as_str()?.to_owned(),
                artist: track["artists"][0]["name"].as_str()?.to_owned(),
                album: track["album"]["name"].as_str().map(str::to_owned),
                album_artist: track["album"]["artists"][0]["name"]
                    .as_str()
                    .map(str::to_owned),
                track_number: track["track_number"].as_i64(),
                track_total: track["album"]["total_tracks"].as_i64(),
                disc_number: track["disc_number"].as_i64(),
                year: release_date
                    .as_deref()
                    .and_then(|date| date.get(..4))
                    .map(str::to_owned),
                release_date,
                release_type: album_type.clone(),
                is_compilation: album_type.as_deref() == Some("compilation"),
                isrc: track["external_ids"]["isrc"].as_str().map(str::to_owned),
                cover_url: track["album"]["images"]
                    .as_array()
                    .and_then(|images| images.first())
                    .and_then(|image| image["url"].as_str())
                    .map(str::to_owned),
                duration_delta: track["duration_ms"].as_f64().map(|value| value / 1000.0),
                raw_json: track.to_string(),
                ..Default::default()
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_isrc_and_release_fields_without_using_musicbrainz_id_columns() {
        let values = parse_results(&serde_json::json!({"tracks": {"items": [{
            "id": "spotify-id", "type": "track", "name": "Song", "duration_ms": 181250,
            "track_number": 3, "disc_number": 1,
            "artists": [{"name": "Artist"}], "external_ids": {"isrc": "US1234567890"},
            "album": {"name": "Album", "album_type": "album", "release_date": "2020-04-03",
                "total_tracks": 10, "artists": [{"name": "Artist"}],
                "images": [{"url": "https://example.test/cover.jpg"}]}
        }]}}));
        assert_eq!(values[0].isrc.as_deref(), Some("US1234567890"));
        assert_eq!(values[0].track_total, Some(10));
        assert_eq!(values[0].year.as_deref(), Some("2020"));
        assert!(values[0].recording_id.is_none());
    }

    #[test]
    fn rejects_non_track_items() {
        assert!(
            parse_results(&serde_json::json!({"tracks":{"items":[{"type":"episode"}]}})).is_empty()
        );
    }

    #[test]
    fn upgrades_spotify_thumbnail_to_large_cover() {
        assert_eq!(
            upgrade_artwork_url("https://i.scdn.co/image/ab67616d00001e02abc"),
            "https://i.scdn.co/image/ab67616d0000b273abc"
        );
    }
}
