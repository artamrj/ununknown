pub mod acoustid;
pub mod cover_art_archive;
pub mod musicbrainz;

use crate::{audio::AudioInfo, config::Config};
use anyhow::Result;
use reqwest::Client;
use serde::{Deserialize, Serialize};

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
pub struct Candidate {
    pub title: String,
    pub artist: String,
    pub album: Option<String>,
    pub album_artist: Option<String>,
    pub track_number: Option<i64>,
    pub track_total: Option<i64>,
    pub disc_number: Option<i64>,
    pub disc_total: Option<i64>,
    pub year: Option<String>,
    pub genre: Option<String>,
    pub composer: Option<String>,
    pub label: Option<String>,
    pub isrc: Option<String>,
    pub cover_url: Option<String>,
    pub recording_id: Option<String>,
    pub release_id: Option<String>,
    pub artist_id: Option<String>,
    pub album_artist_id: Option<String>,
    pub score: f64,
    pub raw_json: String,
}

pub async fn identify(
    client: &Client,
    cfg: &Config,
    fingerprint: &str,
    duration: f64,
    current: &AudioInfo,
) -> Result<Vec<Candidate>> {
    if cfg.acoustid_api_key.is_empty() {
        return Ok(vec![]);
    }
    let hits = acoustid::lookup(client, &cfg.acoustid_api_key, fingerprint, duration).await?;
    let mut out = Vec::new();
    for hit in hits.into_iter().take(3) {
        tokio::time::sleep(std::time::Duration::from_secs(1)).await;
        if let Ok(mut candidate) =
            musicbrainz::recording(client, &cfg.musicbrainz_user_agent, &hit.recording_id).await
        {
            candidate.score = crate::matcher::score(
                hit.score,
                current,
                &candidate.title,
                &candidate.artist,
                duration,
            );
            out.push(candidate);
        }
    }
    Ok(out)
}
