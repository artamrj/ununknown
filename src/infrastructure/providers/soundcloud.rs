use super::Candidate;
use anyhow::{Result, bail};
use base64::{Engine, engine::general_purpose::STANDARD};
use reqwest::Client;
use serde_json::Value;
use std::time::{Duration, Instant};
use tokio::sync::Mutex;

#[derive(Default)]
pub struct SoundCloudAuth {
    token: Mutex<Option<CachedToken>>,
}

struct CachedToken {
    access_token: String,
    refresh_token: Option<String>,
    expires_at: Instant,
}

pub async fn search(
    client: &Client,
    auth: &SoundCloudAuth,
    client_id: &str,
    client_secret: &str,
    title: &str,
    artist: Option<&str>,
) -> Result<Vec<Candidate>> {
    let token = access_token(client, auth, client_id, client_secret).await?;
    let query = match artist.filter(|value| !value.trim().is_empty()) {
        Some(artist) => format!("{} {}", artist.trim(), title.trim()),
        None => title.trim().to_owned(),
    };
    let raw = client
        .get("https://api.soundcloud.com/tracks")
        .header("Authorization", format!("OAuth {token}"))
        .query(&[
            ("q", query.as_str()),
            ("limit", "15"),
            ("linked_partitioning", "true"),
        ])
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    Ok(parse_results(&raw))
}

pub async fn lookup_url(client: &Client, url: &str) -> Result<Candidate> {
    validate_url(url)?;
    let raw = client
        .get("https://soundcloud.com/oembed")
        .query(&[("url", url), ("format", "json")])
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    candidate_from_oembed(&raw).ok_or_else(|| anyhow::anyhow!("SoundCloud returned no title"))
}

fn validate_url(url: &str) -> Result<()> {
    let parsed = reqwest::Url::parse(url)?;
    if parsed.scheme() != "https"
        || !matches!(
            parsed.host_str(),
            Some("soundcloud.com" | "www.soundcloud.com")
        )
    {
        bail!("only HTTPS SoundCloud source links are supported");
    }
    Ok(())
}

async fn access_token(
    client: &Client,
    auth: &SoundCloudAuth,
    client_id: &str,
    client_secret: &str,
) -> Result<String> {
    let mut cached = auth.token.lock().await;
    if let Some(token) = cached.as_ref()
        && token.expires_at > Instant::now() + Duration::from_secs(30)
    {
        return Ok(token.access_token.clone());
    }

    let refreshed = if let Some(refresh_token) = cached
        .as_ref()
        .and_then(|token| token.refresh_token.as_deref())
    {
        request_token(
            client,
            client_id,
            client_secret,
            &[
                ("grant_type", "refresh_token"),
                ("client_id", client_id),
                ("client_secret", client_secret),
                ("refresh_token", refresh_token),
            ],
            false,
        )
        .await
        .ok()
    } else {
        None
    };
    let token = match refreshed {
        Some(token) => token,
        None => {
            request_token(
                client,
                client_id,
                client_secret,
                &[("grant_type", "client_credentials")],
                true,
            )
            .await?
        }
    };
    let access_token = token.access_token.clone();
    *cached = Some(token);
    Ok(access_token)
}

async fn request_token(
    client: &Client,
    client_id: &str,
    client_secret: &str,
    form: &[(&str, &str)],
    basic_auth: bool,
) -> Result<CachedToken> {
    let mut request = client
        .post("https://secure.soundcloud.com/oauth/token")
        .header("Accept", "application/json; charset=utf-8")
        .form(form);
    if basic_auth {
        request = request.header(
            "Authorization",
            format!(
                "Basic {}",
                STANDARD.encode(format!("{client_id}:{client_secret}"))
            ),
        );
    }
    let raw = request
        .send()
        .await?
        .error_for_status()?
        .json::<Value>()
        .await?;
    let access_token = raw["access_token"]
        .as_str()
        .ok_or_else(|| anyhow::anyhow!("SoundCloud token response had no access token"))?
        .to_owned();
    let expires_in = raw["expires_in"].as_u64().unwrap_or(3600).max(60);
    Ok(CachedToken {
        access_token,
        refresh_token: raw["refresh_token"].as_str().map(str::to_owned),
        expires_at: Instant::now() + Duration::from_secs(expires_in),
    })
}

fn parse_results(raw: &Value) -> Vec<Candidate> {
    raw["collection"]
        .as_array()
        .or_else(|| raw.as_array())
        .into_iter()
        .flatten()
        .filter_map(candidate_from_track)
        .collect()
}

fn candidate_from_track(track: &Value) -> Option<Candidate> {
    if track["kind"].as_str().is_some_and(|kind| kind != "track") {
        return None;
    }
    let raw_title = track["title"].as_str()?.trim();
    let explicit_artist = nonempty(track["metadata_artist"].as_str());
    let title_credit = split_artist_title(raw_title);
    let use_title_credit = title_credit.as_ref().is_some_and(|(title_artist, _)| {
        explicit_artist
            .as_deref()
            .is_none_or(|artist| same_credit(title_artist, artist))
    });
    let title = title_credit
        .as_ref()
        .filter(|_| use_title_credit)
        .map_or(raw_title, |(_, title)| title);
    let artist = explicit_artist
        .or_else(|| {
            title_credit
                .as_ref()
                .filter(|_| use_title_credit)
                .map(|(artist, _)| artist.clone())
        })
        .or_else(|| nonempty(track["user"]["username"].as_str()))?;
    let credits = crate::domain::credits::normalize_featured(&artist, title);
    let release_year = track["release_year"].as_i64().map(|year| year.to_string());
    let release_date = release_year.as_deref().map(|year| {
        match (
            track["release_month"].as_u64(),
            track["release_day"].as_u64(),
        ) {
            (Some(month), Some(day)) => format!("{year}-{month:02}-{day:02}"),
            (Some(month), None) => format!("{year}-{month:02}"),
            _ => year.to_owned(),
        }
    });
    let album_artist = credits.artist.clone();
    Some(Candidate {
        provider: "soundcloud".into(),
        title: credits.title,
        artist: credits.artist,
        album: track["release"].as_str().map(str::to_owned),
        album_artist: Some(album_artist),
        year: release_year,
        genre: nonempty(track["genre"].as_str()),
        label: nonempty(track["label_name"].as_str()),
        isrc: nonempty(track["isrc"].as_str()),
        cover_url: track["artwork_url"].as_str().map(upgrade_artwork_url),
        release_date,
        duration_delta: track["duration"].as_f64().map(|value| value / 1000.0),
        raw_json: track.to_string(),
        ..Default::default()
    })
}

fn candidate_from_oembed(raw: &Value) -> Option<Candidate> {
    let raw_title = raw["title"].as_str()?.trim();
    let author = raw["author_name"].as_str().unwrap_or("SoundCloud artist");
    let suffix = format!(" by {author}");
    let cleaned = raw_title.strip_suffix(&suffix).unwrap_or(raw_title).trim();
    let (artist, title) =
        split_artist_title(cleaned).unwrap_or_else(|| (author.to_owned(), cleaned.to_owned()));
    let credits = crate::domain::credits::normalize_featured(&artist, &title);
    Some(Candidate {
        provider: "soundcloud".into(),
        title: credits.title,
        artist: credits.artist,
        cover_url: raw["thumbnail_url"].as_str().map(upgrade_artwork_url),
        score: 94.0,
        score_breakdown: Some(
            serde_json::json!({
                "source": "soundcloud_user_source_url",
                "sources": ["SoundCloud"],
                "user_verified_source": true
            })
            .to_string(),
        ),
        raw_json: raw.to_string(),
        ..Default::default()
    })
}

fn nonempty(value: Option<&str>) -> Option<String> {
    value
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn split_artist_title(value: &str) -> Option<(String, String)> {
    [" - ", " – ", " — ", " | "]
        .into_iter()
        .find_map(|separator| {
            let (artist, title) = value.split_once(separator)?;
            (!artist.trim().is_empty() && !title.trim().is_empty())
                .then(|| (artist.trim().to_owned(), title.trim().to_owned()))
        })
}

fn same_credit(left: &str, right: &str) -> bool {
    let normalize = |value: &str| {
        value
            .chars()
            .filter(|character| character.is_alphanumeric())
            .flat_map(char::to_lowercase)
            .collect::<String>()
    };
    normalize(left) == normalize(right)
}

pub fn upgrade_artwork_url(url: &str) -> String {
    const SIZES: [&str; 7] = [
        "-large.",
        "-t300x300.",
        "-t200x200.",
        "-t120x120.",
        "-t67x67.",
        "-small.",
        "-tiny.",
    ];
    SIZES
        .into_iter()
        .fold(url.to_owned(), |url, size| url.replace(size, "-t500x500."))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_search_metadata_and_upgrades_cover() {
        let candidates = parse_results(&serde_json::json!({"collection": [{
            "kind": "track", "title": "Song (ft Guest)", "metadata_artist": "Artist",
            "duration": 203000, "genre": "Persian Pop", "isrc": "US1234567890",
            "release": "Single", "release_year": 2024, "release_month": 3,
            "release_day": 2, "artwork_url": "https://i1.sndcdn.com/artworks-id-large.jpg",
            "user": {"username": "Uploader"}
        }]}));
        assert_eq!(candidates[0].provider, "soundcloud");
        assert_eq!(candidates[0].artist, "Artist");
        assert_eq!(candidates[0].title, "Song (feat. Guest)");
        assert_eq!(candidates[0].release_date.as_deref(), Some("2024-03-02"));
        assert_eq!(
            candidates[0].cover_url.as_deref(),
            Some("https://i1.sndcdn.com/artworks-id-t500x500.jpg")
        );
    }

    #[test]
    fn parses_user_supplied_soundcloud_example() {
        let candidate = candidate_from_oembed(&serde_json::json!({
            "title": "Arta - Hanooz Yadame (Ft Koorosh, Sami Low, & Raha) by Sina Mohtasham",
            "author_name": "Sina Mohtasham",
            "thumbnail_url": "https://i1.sndcdn.com/artworks-id-t500x500.jpg"
        }))
        .unwrap();
        assert_eq!(candidate.artist, "Arta");
        assert_eq!(
            candidate.title,
            "Hanooz Yadame (feat. Koorosh, Sami Low, & Raha)"
        );
        assert_eq!(candidate.score, 94.0);
    }

    #[test]
    fn title_credit_beats_an_unrelated_uploader_name() {
        let candidate = candidate_from_track(&serde_json::json!({
            "kind": "track",
            "title": "Arta - Hanooz Yadame (Ft Koorosh)",
            "user": {"username": "Sina Mohtasham"},
            "artwork_url": "https://i1.sndcdn.com/artworks-id-large.jpg"
        }))
        .unwrap();
        assert_eq!(candidate.artist, "Arta");
        assert_eq!(candidate.title, "Hanooz Yadame (feat. Koorosh)");
    }
}
