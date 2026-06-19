use super::Candidate;
use crate::{
    domain::audio::AudioInfo,
    infrastructure::provider_cache::{ProviderCache, search_key},
};
use anyhow::Result;
use chrono::{Duration, Utc};
use reqwest::Client;
use serde_json::Value;
use sqlx::SqlitePool;

pub async fn search(
    pool: &SqlitePool,
    client: &Client,
    api_key: &str,
    current: &AudioInfo,
) -> Result<Vec<Candidate>> {
    let Some(title) = current
        .title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(Vec::new());
    };
    let artist = current.artist.as_deref().unwrap_or_default();
    let key = search_key(&format!("{artist} {title}"));
    let raw = if let Some(value) = ProviderCache::get(pool, "theaudiodb", &key).await? {
        value
    } else {
        let value: Value = client
            .get(format!(
                "https://www.theaudiodb.com/api/v1/json/{api_key}/searchtrack.php"
            ))
            .query(&[("s", artist), ("t", title)])
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        ProviderCache::put(
            pool,
            "theaudiodb",
            &key,
            &value,
            Utc::now() + Duration::days(14),
        )
        .await?;
        value
    };
    Ok(candidates_from_search(&raw))
}

fn candidates_from_search(raw: &Value) -> Vec<Candidate> {
    raw["track"]
        .as_array()
        .into_iter()
        .flatten()
        .take(5)
        .map(|track| Candidate {
            provider: "theaudiodb".into(),
            title: track["strTrack"].as_str().unwrap_or("Unknown Title").into(),
            artist: track["strArtist"]
                .as_str()
                .unwrap_or("Unknown Artist")
                .into(),
            album: track["strAlbum"].as_str().map(str::to_owned),
            genre: track["strGenre"].as_str().map(str::to_owned),
            year: track["intYear"].as_str().map(str::to_owned),
            cover_url: track["strTrackThumb"]
                .as_str()
                .or_else(|| track["strAlbumThumb"].as_str())
                .map(str::to_owned),
            recording_id: track["strMusicBrainzID"]
                .as_str()
                .filter(|value| !value.is_empty())
                .map(str::to_owned),
            raw_json: track.to_string(),
            ..Default::default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_theaudiodb_track() {
        let raw = serde_json::json!({
            "track": [{"strTrack": "Song", "strArtist": "Artist", "strAlbum": "Album", "strGenre": "Rock"}]
        });
        let candidates = candidates_from_search(&raw);
        assert_eq!(candidates[0].provider, "theaudiodb");
        assert_eq!(candidates[0].genre.as_deref(), Some("Rock"));
    }
}
