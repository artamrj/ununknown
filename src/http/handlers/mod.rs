use crate::{
    core::AppState,
    http::error::{ApiError, ApiResult},
    media::tags,
    providers::Candidate,
    types::{CandidateId, TrackId, WorkflowPhase},
    workers::scan as scan_pipeline,
};
use anyhow::Result;
use axum::{
    Json,
    extract::{Path, State},
};
use serde::{Deserialize, Serialize};
use std::{path::PathBuf, sync::Arc};

pub(super) use crate::db::queries::{CandidateRow, Track};

mod apply;
mod queries;
mod scan;
mod settings;
mod tracks;
mod workspace;

pub use apply::start_apply;
pub use scan::{retry_issues, run_automatic_cycle, start_scan, stop_scan};
pub use settings::{setup, update_setup};
pub use tracks::{
    artwork_preview, auto_approve_review, candidate_artwork_preview, list_tracks, manual_candidate,
    original_artwork_preview, remove_track, resolve_source, return_to_review, search_artwork,
    select_candidate, track_audio, update_artwork,
};
pub use workspace::{frontend_activity, workspace};

#[derive(Deserialize)]
pub struct SelectRequest {
    candidate_id: Option<CandidateId>,
}

#[derive(Deserialize)]
pub struct CandidateEdit {
    title: String,
    artist: String,
    #[serde(default)]
    artist_credits: Vec<crate::domain::credits::ArtistCredit>,
    album: Option<String>,
    album_artist: Option<String>,
    #[serde(default)]
    album_artist_credits: Vec<crate::domain::credits::ArtistCredit>,
    track_number: Option<i64>,
    track_total: Option<i64>,
    disc_number: Option<i64>,
    disc_total: Option<i64>,
    year: Option<String>,
    genre: Option<String>,
    composer: Option<String>,
    label: Option<String>,
    isrc: Option<String>,
    release_date: Option<String>,
    cover_url: Option<String>,
}

#[derive(Deserialize)]
pub struct ArtworkEdit {
    cover_url: String,
}

#[derive(Deserialize)]
pub struct SourceLookupRequest {
    url: String,
}

#[derive(Deserialize)]
pub struct SetupRequest {
    input_dir: String,
    output_dir: String,
    delete_source_after_write: Option<bool>,
    automatic_scan_enabled: Option<bool>,
    automatic_scan_interval_minutes: Option<u64>,
    acoustid_key: Option<String>,
    audd_token: Option<String>,
    spotify_client_id: Option<String>,
    spotify_client_secret: Option<String>,
    soundcloud_client_id: Option<String>,
    soundcloud_client_secret: Option<String>,
    youtube_api_key: Option<String>,
    discogs_token: Option<String>,
    lastfm_key: Option<String>,
    theaudiodb_key: Option<String>,
}

#[derive(Serialize)]
pub struct WorkspaceTrack {
    #[serde(flatten)]
    track: Track,
    candidates: Vec<Candidate>,
}

#[derive(Serialize)]
pub struct TrackPage {
    items: Vec<WorkspaceTrack>,
    total: i64,
}

#[derive(Serialize)]
pub struct AutoApproveResult {
    approved: u64,
    remaining: i64,
    low_confidence: u64,
    unavailable: u64,
}
