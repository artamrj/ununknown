use crate::{
    app::AppState,
    application::scan_pipeline,
    config::Config,
    http::error::{ApiError, ApiResult},
    infrastructure::{media::tag_writer, providers::Candidate},
    types::{CandidateId, TrackId, TrackStage, WorkflowPhase},
};
use anyhow::{Result, anyhow};
use axum::{
    Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::{path::PathBuf, sync::Arc};

mod apply;
mod queries;
mod scan;
mod settings;
mod tracks;
mod workspace;

pub use apply::start_apply;
pub use scan::{start_scan, stop_scan};
pub use settings::{setup, update_setup};
pub use tracks::{list_tracks, manual_candidate, select_candidate};
pub use workspace::workspace;

#[derive(Serialize, FromRow)]
pub struct Track {
    id: TrackId,
    path: String,
    output_path: Option<String>,
    filename: String,
    format: Option<String>,
    duration: Option<f64>,
    current_title: Option<String>,
    current_artist: Option<String>,
    current_album: Option<String>,
    current_album_artist: Option<String>,
    current_track_number: Option<i64>,
    selected_candidate_id: Option<CandidateId>,
    status: String,
    error: Option<String>,
    is_missing: bool,
    stage: TrackStage,
    stage_message: Option<String>,
    retry_count: i64,
    next_retry_at: Option<String>,
}

#[derive(Serialize, FromRow)]
pub struct CandidateRow {
    id: CandidateId,
    track_id: TrackId,
    provider: String,
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    album_artist: Option<String>,
    track_number: Option<i64>,
    track_total: Option<i64>,
    disc_number: Option<i64>,
    disc_total: Option<i64>,
    year: Option<String>,
    genre: Option<String>,
    composer: Option<String>,
    label: Option<String>,
    isrc: Option<String>,
    cover_url: Option<String>,
    musicbrainz_recording_id: Option<String>,
    musicbrainz_release_id: Option<String>,
    release_country: Option<String>,
    release_date: Option<String>,
    release_type: Option<String>,
    release_secondary_types: Option<String>,
    is_compilation: bool,
    duration_delta: Option<f64>,
    score_breakdown: Option<String>,
    musicbrainz_artist_id: Option<String>,
    musicbrainz_album_artist_id: Option<String>,
    score: f64,
    raw_json: Option<String>,
}

impl CandidateRow {
    pub(super) fn value(&self) -> Candidate {
        Candidate {
            id: Some(self.id.0),
            provider: self.provider.clone(),
            title: self.title.clone().unwrap_or_default(),
            artist: self.artist.clone().unwrap_or_default(),
            album: self.album.clone(),
            album_artist: self.album_artist.clone(),
            track_number: self.track_number,
            track_total: self.track_total,
            disc_number: self.disc_number,
            disc_total: self.disc_total,
            year: self.year.clone(),
            genre: self.genre.clone(),
            composer: self.composer.clone(),
            label: self.label.clone(),
            isrc: self.isrc.clone(),
            cover_url: self.cover_url.clone(),
            recording_id: self.musicbrainz_recording_id.clone(),
            release_id: self.musicbrainz_release_id.clone(),
            release_country: self.release_country.clone(),
            release_date: self.release_date.clone(),
            release_type: self.release_type.clone(),
            release_secondary_types: self.release_secondary_types.clone(),
            is_compilation: self.is_compilation,
            duration_delta: self.duration_delta,
            score_breakdown: self.score_breakdown.clone(),
            artist_id: self.musicbrainz_artist_id.clone(),
            album_artist_id: self.musicbrainz_album_artist_id.clone(),
            score: self.score,
            raw_json: self.raw_json.clone().unwrap_or_default(),
        }
    }
}

#[derive(Clone)]
pub struct PreviewItem {
    track_id: TrackId,
    filename: String,
    current_path: String,
    destination_path: String,
}

#[derive(Deserialize)]
pub struct SelectRequest {
    candidate_id: Option<CandidateId>,
}

#[derive(Deserialize)]
pub struct CandidateEdit {
    title: String,
    artist: String,
    album: Option<String>,
    album_artist: Option<String>,
    track_number: Option<i64>,
    track_total: Option<i64>,
    disc_number: Option<i64>,
    disc_total: Option<i64>,
    year: Option<String>,
    genre: Option<String>,
    composer: Option<String>,
    label: Option<String>,
    isrc: Option<String>,
}

#[derive(Deserialize)]
pub struct SetupRequest {
    input_dir: String,
    output_dir: String,
    acoustid_key: Option<String>,
    discogs_token: Option<String>,
    lastfm_key: Option<String>,
    theaudiodb_key: Option<String>,
}

#[derive(Serialize)]
pub struct WorkspaceTrack {
    #[serde(flatten)]
    track: Track,
    candidates: Vec<CandidateRow>,
}

#[derive(Serialize)]
pub struct TrackPage {
    items: Vec<WorkspaceTrack>,
    total: i64,
}

fn destination(cfg: &Config, track: &Track, _candidate: &Candidate) -> Result<String> {
    let source = std::path::Path::new(&track.path);
    let relative = source
        .strip_prefix(&cfg.input_dir)
        .ok()
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| source.file_name().map(std::path::Path::new))
        .ok_or_else(|| anyhow!("audio file has no filename"))?;
    Ok(PathBuf::from(&cfg.output_dir)
        .join(relative)
        .to_string_lossy()
        .into_owned())
}
