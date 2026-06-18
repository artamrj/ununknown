use crate::{
    app::{AppState, Workflow},
    application::scan_pipeline,
    config::Config,
    domain::path_templates::{self, TemplateValues},
    http::error::ApiResult,
    infrastructure::{media::tag_writer, providers::Candidate},
    jobs,
    types::{
        CandidateId, DuplicateAction, JobId, OutputMode, PreviewToken, TrackId, TrackStage,
        WorkflowPhase,
    },
};
use anyhow::{Result, anyhow};
use axum::{
    Json,
    body::Body,
    extract::{Path, Query, State},
    http::{StatusCode, header},
    response::{IntoResponse, Response, Sse, sse::Event},
};
use chrono::Utc;
use futures::Stream;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use std::{collections::HashMap, convert::Infallible, path::PathBuf, sync::Arc};

mod apply;
mod artwork;
mod events;
mod scan;
mod settings;
mod tracks;
mod workspace;

pub use apply::{apply_preview, start_apply, stop_apply, template_preview};
pub use artwork::{current_artwork, proposed_artwork};
pub use events::events;
pub use scan::{get_job, list_jobs, start_scan, stop_scan};
pub use settings::{
    reset_settings, reset_settings_section, settings, test_acoustid, test_musicbrainz,
    update_settings,
};
pub use tracks::{
    candidates, edit_candidate, get_track, list_tracks, retry_failed, retry_track,
    select_candidate, skip_review,
};
pub use workspace::{clear_workspace, workspace};

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
    musicbrainz_artist_id: Option<String>,
    musicbrainz_album_artist_id: Option<String>,
    score: f64,
    raw_json: Option<String>,
}
impl CandidateRow {
    fn value(&self) -> Candidate {
        Candidate {
            id: Some(self.id.0),
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
            artist_id: self.musicbrainz_artist_id.clone(),
            album_artist_id: self.musicbrainz_album_artist_id.clone(),
            score: self.score,
            raw_json: self.raw_json.clone().unwrap_or_default(),
        }
    }
}
#[derive(Clone, Serialize)]
pub struct PreviewItem {
    track_id: TrackId,
    candidate_id: CandidateId,
    filename: String,
    current_path: String,
    destination_path: String,
    action: String,
    warnings: Vec<String>,
    duplicate_group_id: Option<String>,
    duplicate_action: DuplicateAction,
    duplicate_reason: Option<String>,
    kept_track_id: Option<TrackId>,
    old: MetadataSummary,
    new: MetadataSummary,
    cover_url: Option<String>,
    current_cover_url: Option<String>,
    proposed_cover_url: Option<String>,
    confidence: f64,
    artwork_action: String,
}
#[derive(Clone, Serialize)]
pub struct MetadataSummary {
    title: Option<String>,
    artist: Option<String>,
    album: Option<String>,
    album_artist: Option<String>,
    track_number: Option<i64>,
    disc_number: Option<i64>,
    year: Option<String>,
    genre: Option<String>,
    label: Option<String>,
    isrc: Option<String>,
    duration: Option<f64>,
    format: Option<String>,
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
pub struct PreviewRequest {
    template: Option<String>,
    track_id: Option<TrackId>,
    settings: Option<Config>,
}
#[derive(Deserialize)]
pub struct ApplyRequest {
    preview_token: PreviewToken,
}
#[derive(Serialize)]
pub struct PathPreviewResult {
    label: String,
    template: String,
    path: Option<String>,
    warnings: Vec<String>,
    errors: Vec<String>,
}
#[derive(Deserialize)]
pub struct SettingsRequest {
    #[serde(flatten)]
    config: Config,
}
#[derive(Serialize)]
pub struct WorkspaceTrack {
    #[serde(flatten)]
    track: Track,
    candidates: Vec<CandidateRow>,
}
#[derive(Deserialize)]
pub struct TrackQuery {
    page: Option<i64>,
    page_size: Option<i64>,
    status: Option<String>,
    search: Option<String>,
}
#[derive(Serialize)]
pub struct TrackPage {
    items: Vec<WorkspaceTrack>,
    total: i64,
    counts: HashMap<String, i64>,
}

fn destination(
    cfg: &Config,
    track: &Track,
    c: &Candidate,
    template: Option<&str>,
) -> Result<String> {
    if cfg.output_mode == OutputMode::InPlace
        && !cfg.in_place.rename_files
        && !cfg.in_place.rename_folders
    {
        return Ok(track.path.clone());
    }
    let ext = track.format.clone().unwrap_or_default();
    let values = TemplateValues {
        artist: Some(c.artist.clone()),
        albumartist: c.album_artist.clone(),
        album: c.album.clone(),
        title: Some(c.title.clone()),
        track: c.track_number,
        tracktotal: c.track_total,
        disc: c.disc_number,
        disctotal: c.disc_total,
        year: c.year.clone(),
        genre: c.genre.clone(),
        composer: c.composer.clone(),
        isrc: c.isrc.clone(),
        label: c.label.clone(),
        format: Some(ext.to_uppercase()),
        bitrate: None,
        ext,
    };
    let compilation = c
        .album_artist
        .as_deref()
        .is_some_and(|v| v.eq_ignore_ascii_case("Various Artists"));
    let chosen = template.unwrap_or(if compilation && cfg.output_mode == OutputMode::Copy {
        &cfg.path_templates.compilation_template
    } else if cfg.output_mode == OutputMode::InPlace && !cfg.in_place.rename_folders {
        &cfg.in_place.filename_template
    } else {
        &cfg.path_templates.default_template
    });
    let relative = path_templates::render(chosen, &values, &cfg.path_templates)?;
    let root = if cfg.output_mode == OutputMode::Copy {
        PathBuf::from(&cfg.output_dir)
    } else if !cfg.in_place.rename_folders {
        PathBuf::from(&track.path)
            .parent()
            .unwrap_or_else(|| std::path::Path::new(&cfg.input_dir))
            .to_path_buf()
    } else {
        PathBuf::from(&cfg.input_dir)
    };
    Ok(path_templates::resolve_collision(
        &root.join(relative),
        cfg.path_templates.collision_strategy,
    )?
    .to_string_lossy()
    .into())
}

fn path_preview_result(
    cfg: &Config,
    track: &Track,
    candidate: &Candidate,
    label: &str,
    template: String,
) -> PathPreviewResult {
    let mut warnings = Vec::new();
    let mut errors = Vec::new();
    if template.trim().is_empty() {
        errors.push("Template cannot be empty".into());
    }
    if template.contains("..") {
        warnings.push("Parent path segments are rejected".into());
    }
    let path = if errors.is_empty() {
        match destination(cfg, track, candidate, Some(&template)) {
            Ok(path) => {
                if !path.to_ascii_lowercase().ends_with(".mp3") {
                    warnings.push("Original extension is preserved automatically".into());
                }
                Some(path)
            }
            Err(error) => {
                errors.push(error.to_string());
                None
            }
        }
    } else {
        None
    };
    PathPreviewResult {
        label: label.into(),
        template,
        path,
        warnings,
        errors,
    }
}

fn old_summary(track: &Track) -> MetadataSummary {
    MetadataSummary {
        title: track.current_title.clone(),
        artist: track.current_artist.clone(),
        album: track.current_album.clone(),
        album_artist: track.current_album_artist.clone(),
        track_number: track.current_track_number,
        disc_number: None,
        year: None,
        genre: None,
        label: None,
        isrc: None,
        duration: track.duration,
        format: track.format.clone(),
    }
}

fn new_summary(candidate: &Candidate, track: &Track) -> MetadataSummary {
    MetadataSummary {
        title: Some(candidate.title.clone()),
        artist: Some(candidate.artist.clone()),
        album: candidate.album.clone(),
        album_artist: candidate.album_artist.clone(),
        track_number: candidate.track_number,
        disc_number: candidate.disc_number,
        year: candidate.year.clone(),
        genre: candidate.genre.clone(),
        label: candidate.label.clone(),
        isrc: candidate.isrc.clone(),
        duration: track.duration,
        format: track.format.clone(),
    }
}

#[derive(Clone, Default)]
struct DuplicateResolution {
    duplicate_group_id: Option<String>,
    duplicate_action: DuplicateAction,
    duplicate_reason: Option<String>,
    kept_track_id: Option<TrackId>,
}

async fn load_selected(pool: &SqlitePool, tracks: Vec<Track>) -> Result<Vec<(Track, Candidate)>> {
    let mut out = Vec::with_capacity(tracks.len());
    for track in tracks {
        let (_, candidate) = selected(pool, track.id).await?;
        out.push((track, candidate));
    }
    Ok(out)
}

fn duplicate_actions(selected: &[(Track, Candidate)]) -> HashMap<TrackId, DuplicateResolution> {
    let mut groups: HashMap<String, Vec<usize>> = HashMap::new();
    for (index, (track, candidate)) in selected.iter().enumerate() {
        groups
            .entry(duplicate_key(track, candidate))
            .or_default()
            .push(index);
    }
    let mut actions = HashMap::new();
    for (key, indexes) in groups.into_iter().filter(|(_, indexes)| indexes.len() > 1) {
        let keep_index = *indexes
            .iter()
            .max_by(|a, b| {
                let (ta, ca) = &selected[**a];
                let (tb, cb) = &selected[**b];
                ca.score
                    .total_cmp(&cb.score)
                    .then_with(|| {
                        ta.duration
                            .unwrap_or_default()
                            .total_cmp(&tb.duration.unwrap_or_default())
                    })
                    .then_with(|| tb.path.cmp(&ta.path))
            })
            .expect("duplicate group has indexes");
        let keep_id = selected[keep_index].0.id;
        for index in indexes {
            let (track, candidate) = &selected[index];
            let reason = if track.id == keep_id {
                format!(
                    "Keeping best duplicate match at {:.0}% confidence",
                    candidate.score
                )
            } else {
                format!("Duplicate of track {keep_id}; kept the stronger match")
            };
            actions.insert(
                track.id,
                DuplicateResolution {
                    duplicate_group_id: Some(key.clone()),
                    duplicate_action: if track.id == keep_id {
                        DuplicateAction::Keep
                    } else {
                        DuplicateAction::SkipDuplicate
                    },
                    duplicate_reason: Some(reason),
                    kept_track_id: Some(keep_id),
                },
            );
        }
    }
    actions
}

fn duplicate_key(track: &Track, candidate: &Candidate) -> String {
    if let Some(id) = candidate
        .recording_id
        .as_deref()
        .filter(|v| !v.trim().is_empty())
    {
        return format!("mbid:{}", id.trim().to_ascii_lowercase());
    }
    if let Some(isrc) = candidate.isrc.as_deref().filter(|v| !v.trim().is_empty()) {
        return format!("isrc:{}", isrc.trim().to_ascii_uppercase());
    }
    let duration_bucket = track.duration.unwrap_or_default().round() as i64 / 3;
    format!(
        "text:{}:{}:{}",
        normalize_duplicate_text(&candidate.artist),
        normalize_duplicate_text(&candidate.title),
        duration_bucket
    )
}

fn normalize_duplicate_text(value: &str) -> String {
    value
        .chars()
        .filter(|c| c.is_ascii_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

async fn selected(pool: &SqlitePool, id: TrackId) -> Result<(Track, Candidate)> {
    let track:Track=sqlx::query_as("SELECT id,path,output_path,filename,format,duration,current_title,current_artist,current_album,current_album_artist,current_track_number,selected_candidate_id,status,error,is_missing,stage,stage_message,retry_count,next_retry_at FROM tracks WHERE id=?").bind(id.0).fetch_one(pool).await?;
    let cid = track
        .selected_candidate_id
        .ok_or_else(|| anyhow!("track has no selected candidate"))?;
    let row: CandidateRow = sqlx::query_as("SELECT * FROM candidates WHERE id=?")
        .bind(cid.0)
        .fetch_one(pool)
        .await?;
    Ok((track, row.value()))
}
async fn fetch_candidates(pool: &SqlitePool, id: TrackId) -> Result<Vec<CandidateRow>> {
    Ok(
        sqlx::query_as("SELECT * FROM candidates WHERE track_id=? ORDER BY score DESC")
            .bind(id.0)
            .fetch_all(pool)
            .await?,
    )
}
