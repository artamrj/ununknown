use super::Candidate;
use crate::{
    domain::audio::AudioInfo,
    infrastructure::provider_cache::{ProviderCache, release_key, search_key},
};
use anyhow::Result;
use chrono::{Duration, Utc};
use reqwest::{Client, RequestBuilder};
use serde_json::Value;
use sqlx::SqlitePool;

pub async fn search(
    pool: &SqlitePool,
    client: &Client,
    token: Option<&str>,
    current: &AudioInfo,
) -> Result<Vec<Candidate>> {
    let Some(title) = current
        .title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(Vec::new());
    };
    let query = format!(
        "{} {}",
        current.artist.as_deref().unwrap_or_default(),
        title
    );
    let key = search_key(&query);
    let raw = if let Some(value) = ProviderCache::get(pool, "discogs", &key).await? {
        value
    } else {
        let mut request = client
            .get("https://api.discogs.com/database/search")
            .query(&[("type", "release"), ("track", title), ("q", query.as_str())])
            .header("User-Agent", "Ununknown/0.6.0");
        if let Some(token) = token.filter(|value| !value.trim().is_empty()) {
            request = request.bearer_auth(token);
        }
        let value = request_json(request).await?;
        ProviderCache::put(
            pool,
            "discogs",
            &key,
            &value,
            Utc::now() + Duration::days(14),
        )
        .await?;
        value
    };

    let mut out = Vec::new();
    for result in raw["results"].as_array().into_iter().flatten().take(5) {
        let Some(id) = result["id"].as_i64() else {
            continue;
        };
        let release = fetch_release(pool, client, token, id)
            .await
            .unwrap_or_else(|_| result.clone());
        out.extend(candidates_from_release(&release, result, current));
        if out.len() >= 5 {
            break;
        }
    }
    Ok(out)
}

async fn fetch_release(
    pool: &SqlitePool,
    client: &Client,
    token: Option<&str>,
    id: i64,
) -> Result<Value> {
    let key = release_key(&id.to_string());
    if let Some(value) = ProviderCache::get(pool, "discogs_release", &key).await? {
        return Ok(value);
    }
    let mut request = client
        .get(format!("https://api.discogs.com/releases/{id}"))
        .header("User-Agent", "Ununknown/0.6.0");
    if let Some(token) = token.filter(|value| !value.trim().is_empty()) {
        request = request.bearer_auth(token);
    }
    let value = request_json(request).await?;
    ProviderCache::put(
        pool,
        "discogs_release",
        &key,
        &value,
        Utc::now() + Duration::days(30),
    )
    .await?;
    Ok(value)
}

fn candidates_from_release(
    release: &Value,
    search_result: &Value,
    current: &AudioInfo,
) -> Vec<Candidate> {
    let target_title = current.title.as_deref().unwrap_or_default();
    let artist = release["artists_sort"]
        .as_str()
        .or_else(|| {
            search_result["title"]
                .as_str()
                .and_then(|value| value.split(" - ").next())
        })
        .unwrap_or("Unknown Artist");
    let album = release["title"].as_str().or_else(|| {
        search_result["title"]
            .as_str()
            .and_then(|value| value.split(" - ").nth(1))
    });
    let label = release["labels"]
        .as_array()
        .and_then(|labels| labels.first())
        .and_then(|label| label["name"].as_str())
        .map(str::to_owned);
    let country = release["country"].as_str().map(str::to_owned);
    let year = release["year"].as_i64().map(|value| value.to_string());
    let cover_url = release["images"]
        .as_array()
        .and_then(|images| images.first())
        .and_then(|image| {
            image["resource_url"]
                .as_str()
                .or_else(|| image["uri"].as_str())
        })
        .map(str::to_owned)
        .or_else(|| search_result["cover_image"].as_str().map(str::to_owned));
    let release_id = release["id"]
        .as_i64()
        .map(|value| format!("discogs:{value}"));
    let tracklist = release["tracklist"].as_array();
    let mut out = Vec::new();
    if let Some(tracks) = tracklist {
        for (index, track) in tracks.iter().enumerate() {
            let title = track["title"].as_str().unwrap_or_default();
            if !target_title.is_empty()
                && strsim::normalized_levenshtein(
                    &title.to_ascii_lowercase(),
                    &target_title.to_ascii_lowercase(),
                ) < 0.55
            {
                continue;
            }
            out.push(Candidate {
                provider: "discogs".into(),
                title: title.to_owned(),
                artist: artist.to_owned(),
                album: album.map(str::to_owned),
                track_number: Some((index + 1) as i64),
                track_total: Some(tracks.len() as i64),
                year: year.clone(),
                label: label.clone(),
                cover_url: cover_url.clone(),
                release_id: release_id.clone(),
                release_country: country.clone(),
                release_date: year.clone(),
                release_type: Some("Release".into()),
                raw_json: release.to_string(),
                ..Default::default()
            });
        }
    }
    if out.is_empty() {
        out.push(Candidate {
            provider: "discogs".into(),
            title: target_title.to_owned(),
            artist: artist.to_owned(),
            album: album.map(str::to_owned),
            year,
            label,
            cover_url,
            release_id,
            release_country: country,
            release_type: Some("Release".into()),
            raw_json: release.to_string(),
            ..Default::default()
        });
    }
    out
}

async fn request_json(request: RequestBuilder) -> Result<Value> {
    Ok(request.send().await?.error_for_status()?.json().await?)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_discogs_release_track_candidate() {
        let current = AudioInfo {
            title: Some("Song".into()),
            artist: Some("Artist".into()),
            ..Default::default()
        };
        let raw = serde_json::json!({
            "id": 123,
            "title": "Album",
            "artists_sort": "Artist",
            "country": "US",
            "year": 1999,
            "labels": [{"name": "Label"}],
            "tracklist": [{"title": "Song"}]
        });
        let candidates = candidates_from_release(&raw, &serde_json::json!({}), &current);
        assert_eq!(candidates[0].provider, "discogs");
        assert_eq!(candidates[0].title, "Song");
        assert_eq!(candidates[0].release_country.as_deref(), Some("US"));
    }
}
