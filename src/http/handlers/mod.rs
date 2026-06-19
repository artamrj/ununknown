use crate::{
    app::AppState,
    application::{previews, scan_pipeline},
    config::Config,
    domain::path_templates::{self, TemplateValues},
    http::error::{ApiError, ApiResult},
    infrastructure::{media::tag_writer, providers::Candidate},
    types::{
        CandidateId, DuplicateAction, JobId, OutputMode, PreviewToken, ProviderStatus, TrackId,
        TrackStage, WorkflowPhase,
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
use futures::Stream;
use serde::{Deserialize, Serialize};
use sqlx::FromRow;
use std::{collections::HashMap, convert::Infallible, path::PathBuf, sync::Arc};

mod apply;
mod artwork;
mod events;
mod queries;
mod scan;
mod settings;
mod tracks;
mod workspace;

pub use apply::{apply_preview, start_apply, stop_apply, template_preview};
pub use artwork::{current_artwork, proposed_artwork};
pub use events::events;
pub use scan::{get_job, list_jobs, start_scan, stop_scan};
pub use settings::{
    provider_status, reset_settings, reset_settings_section, settings, test_acoustid,
    test_musicbrainz, test_provider, update_settings,
};
pub use tracks::{
    candidates, edit_candidate, get_track, keep_current_track, list_tracks, retry_failed,
    retry_track, select_candidate, skip_review, skip_track,
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
#[derive(Clone, Serialize, Deserialize)]
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
#[derive(Clone, Serialize, Deserialize)]
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
    view: Option<String>,
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

#[cfg(test)]
mod tests {
    use super::*;
    use axum::extract::{Query, State};
    use sqlx::SqlitePool;
    use std::sync::Arc;

    async fn test_pool() -> SqlitePool {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("test.sqlite");
        let pool = crate::infrastructure::db::connect(path.to_str().unwrap())
            .await
            .unwrap();
        std::mem::forget(dir);
        pool
    }

    fn preview_item(track_id: i64, duplicate_action: DuplicateAction) -> PreviewItem {
        PreviewItem {
            track_id: TrackId(track_id),
            candidate_id: CandidateId(track_id + 100),
            filename: "song.mp3".into(),
            current_path: "/music/in/song.mp3".into(),
            destination_path: "/music/out/song.mp3".into(),
            action: "copy + write tags".into(),
            warnings: Vec::new(),
            duplicate_group_id: None,
            duplicate_action,
            duplicate_reason: None,
            kept_track_id: None,
            old: MetadataSummary {
                title: None,
                artist: None,
                album: None,
                album_artist: None,
                track_number: None,
                disc_number: None,
                year: None,
                genre: None,
                label: None,
                isrc: None,
                duration: None,
                format: Some("mp3".into()),
            },
            new: MetadataSummary {
                title: Some("Song".into()),
                artist: Some("Artist".into()),
                album: None,
                album_artist: None,
                track_number: None,
                disc_number: None,
                year: None,
                genre: None,
                label: None,
                isrc: None,
                duration: None,
                format: Some("mp3".into()),
            },
            cover_url: None,
            current_cover_url: None,
            proposed_cover_url: None,
            confidence: 95.0,
            artwork_action: "no artwork change".into(),
        }
    }

    #[tokio::test]
    async fn preview_persistence_survives_memory_and_consumes_once() {
        let pool = test_pool().await;
        let token = PreviewToken::new();
        let items = vec![
            preview_item(1, DuplicateAction::None),
            preview_item(2, DuplicateAction::SkipDuplicate),
        ];
        previews::store(
            &pool,
            token,
            &items,
            serde_json::json!({"write_count":1,"duplicate_skipped":1}),
            "settings".into(),
            |item| {
                Ok(serde_json::to_string(&item.duplicate_action)?
                    .trim_matches('"')
                    .to_owned())
            },
            |item| item.track_id.0,
            |item| item.candidate_id.0,
        )
        .await
        .unwrap();

        let persisted: i64 = sqlx::query_scalar("SELECT count(*) FROM preview_items")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(persisted, 2);

        let loaded: Vec<PreviewItem> = previews::consume(&pool, token).await.unwrap();
        assert_eq!(loaded.len(), 1);
        assert_eq!(loaded[0].track_id, TrackId(1));
        assert!(
            previews::consume::<PreviewItem>(&pool, token)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn stale_and_missing_preview_tokens_are_rejected() {
        let pool = test_pool().await;
        assert!(
            previews::consume::<PreviewItem>(&pool, PreviewToken::new())
                .await
                .is_err()
        );

        let token = PreviewToken::new();
        let items = vec![preview_item(1, DuplicateAction::None)];
        previews::store(
            &pool,
            token,
            &items,
            serde_json::json!({"write_count":1,"duplicate_skipped":0}),
            "settings".into(),
            |item| {
                Ok(serde_json::to_string(&item.duplicate_action)?
                    .trim_matches('"')
                    .to_owned())
            },
            |item| item.track_id.0,
            |item| item.candidate_id.0,
        )
        .await
        .unwrap();
        previews::invalidate(&pool).await.unwrap();
        assert!(
            previews::consume::<PreviewItem>(&pool, token)
                .await
                .is_err()
        );
    }

    #[tokio::test]
    async fn clear_workspace_preserves_provider_cache() {
        let pool = test_pool().await;
        sqlx::query("INSERT INTO provider_cache(provider,cache_key,response_json,expires_at) VALUES('musicbrainz','k','{}',datetime('now','+1 day'))")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO fingerprint_cache(path,file_size,file_mtime,fingerprint,duration,updated_at) VALUES('/music/a.mp3',10,20,'fp',30,datetime('now'))")
            .execute(&pool)
            .await
            .unwrap();
        let state = Arc::new(AppState::new(Config::default(), pool.clone()));
        let _ = workspace::clear_workspace(State(state)).await.unwrap();
        let cached: i64 = sqlx::query_scalar("SELECT count(*) FROM provider_cache")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(cached, 1);
        let fingerprints: i64 = sqlx::query_scalar("SELECT count(*) FROM fingerprint_cache")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(fingerprints, 1);
    }

    #[tokio::test]
    async fn track_views_filter_unmatched_and_failed_rows() {
        let pool = test_pool().await;
        let now = "2026-06-19T12:00:00Z";
        for (path, filename, stage, selected, status, error) in [
            (
                "/music/selected.mp3",
                "selected.mp3",
                "ready",
                Some(10),
                "selected",
                None,
            ),
            (
                "/music/unmatched.mp3",
                "unmatched.mp3",
                "ready",
                None,
                "selected",
                None,
            ),
            (
                "/music/review.mp3",
                "review.mp3",
                "review",
                None,
                "needs_review",
                None,
            ),
            (
                "/music/failed.mp3",
                "failed.mp3",
                "failed",
                None,
                "provider_error",
                Some("provider failed"),
            ),
        ] {
            sqlx::query("INSERT INTO tracks(path,filename,status,error,first_seen_at,last_seen_at,last_scanned_at,stage,selected_candidate_id) VALUES(?,?,?,?,?,?,?,?,?)")
                .bind(path)
                .bind(filename)
                .bind(status)
                .bind(error)
                .bind(now)
                .bind(now)
                .bind(now)
                .bind(stage)
                .bind(selected)
                .execute(&pool)
                .await
                .unwrap();
        }
        sqlx::query("INSERT INTO candidates(track_id,provider,title,artist,score) SELECT id,'musicbrainz','Review Song','Artist',88 FROM tracks WHERE filename='review.mp3'")
            .execute(&pool)
            .await
            .unwrap();

        let state = Arc::new(AppState::new(Config::default(), pool));
        let unmatched = tracks::list_tracks(
            State(state.clone()),
            Query(TrackQuery {
                page: None,
                page_size: None,
                status: None,
                view: Some("unmatched".into()),
                search: None,
            }),
        )
        .await
        .unwrap()
        .0;
        let review = tracks::list_tracks(
            State(state.clone()),
            Query(TrackQuery {
                page: None,
                page_size: None,
                status: None,
                view: Some("review".into()),
                search: None,
            }),
        )
        .await
        .unwrap()
        .0;
        let failed = tracks::list_tracks(
            State(state),
            Query(TrackQuery {
                page: None,
                page_size: None,
                status: None,
                view: Some("failed".into()),
                search: None,
            }),
        )
        .await
        .unwrap()
        .0;

        let unmatched: Vec<_> = unmatched
            .items
            .into_iter()
            .map(|track| track.track.filename)
            .collect();
        let failed: Vec<_> = failed
            .items
            .into_iter()
            .map(|track| track.track.filename)
            .collect();
        let review: Vec<_> = review
            .items
            .into_iter()
            .map(|track| track.track.filename)
            .collect();
        assert_eq!(unmatched, ["unmatched.mp3"]);
        assert_eq!(review, ["review.mp3"]);
        assert_eq!(failed, ["failed.mp3"]);
    }

    #[tokio::test]
    async fn scan_start_preserves_provider_and_fingerprint_cache() {
        let pool = test_pool().await;
        sqlx::query("INSERT INTO provider_cache(provider,cache_key,response_json,expires_at) VALUES('musicbrainz','k','{}',datetime('now','+1 day'))")
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query("INSERT INTO fingerprint_cache(path,file_size,file_mtime,fingerprint,duration,updated_at) VALUES('/music/a.mp3',10,20,'fp',30,datetime('now'))")
            .execute(&pool)
            .await
            .unwrap();
        let dir = tempfile::tempdir().unwrap();
        let cfg = Config {
            input_dir: dir.path().to_string_lossy().into_owned(),
            ..Default::default()
        };
        let state = Arc::new(AppState::new(cfg, pool.clone()));
        let _ = scan::start_scan(State(state)).await.unwrap();
        let cached: i64 = sqlx::query_scalar("SELECT count(*) FROM provider_cache")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(cached, 1);
        let fingerprints: i64 = sqlx::query_scalar("SELECT count(*) FROM fingerprint_cache")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(fingerprints, 1);
    }

    #[tokio::test]
    async fn invalid_settings_return_422() {
        let pool = test_pool().await;
        let state = Arc::new(AppState::new(Config::default(), pool));
        let mut config = Config::default();
        config.confidence_threshold = 101.0;

        let error = settings::update_settings(State(state), Json(SettingsRequest { config }))
            .await
            .unwrap_err();

        assert_eq!(
            error.into_response().status(),
            StatusCode::UNPROCESSABLE_ENTITY
        );
    }

    #[tokio::test]
    async fn missing_track_returns_404() {
        let pool = test_pool().await;
        let state = Arc::new(AppState::new(Config::default(), pool));

        let error = match tracks::get_track(State(state), Path(TrackId(404))).await {
            Ok(_) => panic!("expected missing track to fail"),
            Err(error) => error,
        };

        assert_eq!(error.into_response().status(), StatusCode::NOT_FOUND);
    }

    #[tokio::test]
    async fn scan_start_while_running_returns_409() {
        let pool = test_pool().await;
        let state = Arc::new(AppState::new(Config::default(), pool));
        state
            .set_workflow(WorkflowPhase::Scan, "scan", "Scanning", 0, 0, None)
            .await;

        let error = scan::start_scan(State(state)).await.unwrap_err();

        assert_eq!(error.into_response().status(), StatusCode::CONFLICT);
    }

    #[tokio::test]
    async fn missing_acoustid_config_returns_403() {
        let pool = test_pool().await;
        let state = Arc::new(AppState::new(Config::default(), pool));

        let error = settings::test_acoustid(State(state)).await.unwrap_err();

        assert_eq!(error.into_response().status(), StatusCode::FORBIDDEN);
    }
}
