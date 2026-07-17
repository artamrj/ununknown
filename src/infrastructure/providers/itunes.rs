use super::Candidate;
use crate::infrastructure::provider_cache::{ProviderCache, search_key};
use anyhow::Result;
use chrono::{Duration, Utc};
use reqwest::Client;
use serde_json::Value;
use sqlx::SqlitePool;

pub async fn search(
    pool: &SqlitePool,
    client: &Client,
    title: &str,
    artist: Option<&str>,
    album: Option<&str>,
) -> Result<Vec<Candidate>> {
    let base_term = artist
        .filter(|value| !value.trim().is_empty())
        .map(|artist| format!("{artist} {title}"))
        .unwrap_or_else(|| title.to_owned());
    let mut terms = vec![base_term.clone(), title.to_owned()];
    if let Some(short_artist) = artist.and_then(|value| value.split_once(" - ").map(|pair| pair.0))
    {
        terms.push(format!("{short_artist} {title}"));
    }
    if let Some(album) = album.filter(|value| !value.trim().is_empty()) {
        terms.push(format!("{base_term} {album}"));
        for segment in album
            .split('|')
            .map(str::trim)
            .filter(|value| !value.is_empty())
        {
            terms.push(segment.to_owned());
            if let Some(short_artist) =
                artist.and_then(|value| value.split_once(" - ").map(|pair| pair.0))
            {
                terms.push(format!("{short_artist} {segment}"));
            }
        }
    }
    terms.sort();
    terms.dedup();
    terms.retain(|term| !term.trim().is_empty());
    let mut out = Vec::new();
    for term in terms {
        out.extend(search_term(pool, client, &term).await?);
    }
    let mut seen = std::collections::HashSet::new();
    out.retain(|candidate| seen.insert(candidate.recording_id.clone()));
    Ok(out)
}

async fn search_term(pool: &SqlitePool, client: &Client, term: &str) -> Result<Vec<Candidate>> {
    let key = search_key(term);
    let raw = if let Some(value) = ProviderCache::get(pool, "itunes", &key).await? {
        value
    } else {
        let value = client
            .get("https://itunes.apple.com/search")
            .query(&[
                ("term", term),
                ("media", "music"),
                ("entity", "song"),
                ("limit", "10"),
            ])
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        ProviderCache::put(pool, "itunes", &key, &value, Utc::now() + Duration::days(7)).await?;
        value
    };
    Ok(parse_results(&raw))
}

fn parse_results(raw: &Value) -> Vec<Candidate> {
    raw["results"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(|value| {
            Some(Candidate {
                provider: "itunes".into(),
                title: value["trackName"].as_str()?.to_owned(),
                artist: value["artistName"].as_str()?.to_owned(),
                album: value["collectionName"].as_str().map(str::to_owned),
                album_artist: value["collectionArtistName"].as_str().map(str::to_owned),
                track_number: value["trackNumber"].as_i64(),
                track_total: value["trackCount"].as_i64(),
                disc_number: value["discNumber"].as_i64(),
                disc_total: value["discCount"].as_i64(),
                year: value["releaseDate"]
                    .as_str()
                    .and_then(|date| date.get(..4))
                    .map(str::to_owned),
                release_date: value["releaseDate"]
                    .as_str()
                    .and_then(|date| date.get(..10))
                    .map(str::to_owned),
                genre: value["primaryGenreName"].as_str().map(str::to_owned),
                cover_url: value["artworkUrl100"]
                    .as_str()
                    .map(|url| url.replace("100x100bb", "1200x1200bb")),
                duration_delta: value["trackTimeMillis"]
                    .as_f64()
                    .map(|value| value / 1000.0),
                recording_id: value["trackId"].as_i64().map(|id| id.to_string()),
                release_id: value["collectionId"].as_i64().map(|id| id.to_string()),
                raw_json: value.to_string(),
                ..Default::default()
            })
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_international_catalog_result() {
        let candidates = parse_results(&serde_json::json!({"results": [{
            "trackId": 1,
            "collectionId": 2,
            "trackName": "Khaar",
            "artistName": "Amir Tataloo",
            "collectionName": "Khaar - Single",
            "trackTimeMillis": 427076,
            "releaseDate": "2019-06-23T12:00:00Z",
            "primaryGenreName": "Pop",
            "artworkUrl100": "https://example.test/100x100bb.jpg",
            "trackNumber": 1,
            "trackCount": 1
        }]}));
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].artist, "Amir Tataloo");
        assert_eq!(candidates[0].release_date.as_deref(), Some("2019-06-23"));
        assert_eq!(candidates[0].duration_delta, Some(427.076));
        assert_eq!(
            candidates[0].cover_url.as_deref(),
            Some("https://example.test/1200x1200bb.jpg")
        );
    }
}
