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
pub use tracks::{
    artwork_preview, candidate_artwork_preview, list_tracks, manual_candidate, resolve_source,
    select_candidate, update_artwork,
};
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
    fn normalized_credits(&self) -> crate::domain::credits::Credits {
        let artist =
            crate::domain::credits::prefer_latin_alias(self.artist.as_deref().unwrap_or_default());
        crate::domain::credits::normalize_featured(
            &artist,
            self.title.as_deref().unwrap_or_default(),
        )
    }

    pub(super) fn normalize_credits(&mut self) {
        let credits = self.normalized_credits();
        self.artist = Some(credits.artist);
        self.title = Some(credits.title);
    }

    pub(super) fn value(&self) -> Candidate {
        let credits = self.normalized_credits();
        Candidate {
            id: Some(self.id.0),
            provider: self.provider.clone(),
            title: credits.title,
            artist: credits.artist,
            album: self.album.clone(),
            album_artist: self
                .album_artist
                .as_deref()
                .map(crate::domain::credits::prefer_latin_alias),
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
    candidates: Vec<CandidateRow>,
}

#[derive(Serialize)]
pub struct TrackPage {
    items: Vec<WorkspaceTrack>,
    total: i64,
}

fn destination(cfg: &Config, track: &Track, candidate: &Candidate) -> Result<String> {
    let source = std::path::Path::new(&track.path);
    let relative = source
        .strip_prefix(&cfg.input_dir)
        .ok()
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| source.file_name().map(std::path::Path::new))
        .ok_or_else(|| anyhow!("audio file has no filename"))?;
    let parent = relative
        .parent()
        .unwrap_or_else(|| std::path::Path::new(""));
    // Re-sniff the source at apply time. The database may still contain an old
    // extension-based format from a previous scan (for example, AAC/M4A bytes in
    // a file named `.mp3`). Falling back keeps previews for missing files usable.
    let detected_format = crate::domain::audio::read(source)
        .ok()
        .map(|info| info.format);
    let extension = detected_format
        .as_deref()
        .filter(|value| !value.is_empty())
        .or_else(|| track.format.as_deref().filter(|value| !value.is_empty()))
        .or_else(|| source.extension().and_then(|value| value.to_str()));
    let credits = crate::domain::credits::normalize_featured(&candidate.artist, &candidate.title);
    let artist = safe_filename_part(&credits.artist, "Unknown Artist");
    let title = safe_filename_part(&credits.title, "Unknown Title");
    let mut basename = truncate_utf8(&format!("{artist} - {title}"), 220).to_owned();
    if let Some(extension) = extension {
        basename.push('.');
        basename.push_str(&extension.to_ascii_lowercase());
    }
    Ok(PathBuf::from(&cfg.output_dir)
        .join(parent)
        .join(basename)
        .to_string_lossy()
        .into_owned())
}

fn safe_filename_part(value: &str, fallback: &str) -> String {
    let cleaned = value
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            {
                ' '
            } else {
                ch
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let cleaned = cleaned.trim_matches([' ', '.']);
    if cleaned.is_empty() {
        fallback.to_owned()
    } else {
        cleaned.to_owned()
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].trim_end()
}

#[cfg(test)]
mod filename_tests {
    use super::*;

    #[test]
    fn filename_parts_preserve_unicode_and_remove_forbidden_characters() {
        assert_eq!(
            safe_filename_part("  فریدون / فرخزاد:  ", "fallback"),
            "فریدون فرخزاد"
        );
        assert_eq!(safe_filename_part("...", "Unknown Title"), "Unknown Title");
    }

    #[test]
    fn utf8_truncation_does_not_split_character() {
        let value = "آهنگ".repeat(100);
        let truncated = truncate_utf8(&value, 220);
        assert!(truncated.len() <= 220);
        assert!(std::str::from_utf8(truncated.as_bytes()).is_ok());
    }
}
