use crate::{
    app::{AppState, Workflow},
    application::scan_pipeline,
    config::Config,
    domain::path_templates::{self, TemplateValues},
    http::error::ApiResult,
    infrastructure::{media::tag_writer, providers::Candidate},
    jobs,
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
use uuid::Uuid;

#[derive(Serialize, FromRow)]
pub struct Track {
    id: i64,
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
    selected_candidate_id: Option<i64>,
    status: String,
    error: Option<String>,
    is_missing: bool,
    stage: String,
    stage_message: Option<String>,
    retry_count: i64,
    next_retry_at: Option<String>,
}
#[derive(Serialize, FromRow)]
pub struct CandidateRow {
    id: i64,
    track_id: i64,
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
            id: Some(self.id),
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
    track_id: i64,
    candidate_id: i64,
    filename: String,
    current_path: String,
    destination_path: String,
    action: String,
    warnings: Vec<String>,
    duplicate_group_id: Option<String>,
    duplicate_action: String,
    duplicate_reason: Option<String>,
    kept_track_id: Option<i64>,
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
    candidate_id: Option<i64>,
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
    track_id: Option<i64>,
    settings: Option<Config>,
}
#[derive(Deserialize)]
pub struct ApplyRequest {
    preview_token: String,
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

pub async fn settings(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::to_value(s.config.read().await.public()).unwrap())
}
pub async fn update_settings(
    State(s): State<Arc<AppState>>,
    Json(body): Json<SettingsRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let current = s.config.read().await.clone();
    let mut cfg = body.config;
    cfg.acoustid_api_key = current.acoustid_api_key;
    cfg.musicbrainz_user_agent = current.musicbrainz_user_agent;
    cfg.db_path = current.db_path;
    cfg.validate()?;
    crate::infrastructure::db::save_settings(&s.pool, &cfg).await?;
    *s.config.write().await = cfg;
    s.previews.write().await.clear();
    Ok(Json(serde_json::json!({"saved":true})))
}
pub async fn reset_settings(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    let current = s.config.read().await.clone();
    let mut cfg = Config {
        db_path: current.db_path,
        input_dir: current.input_dir,
        output_dir: current.output_dir,
        ..Default::default()
    };
    cfg.acoustid_api_key = current.acoustid_api_key;
    crate::infrastructure::db::save_settings(&s.pool, &cfg).await?;
    *s.config.write().await = cfg;
    s.previews.write().await.clear();
    Ok(Json(serde_json::json!({"reset":true})))
}
pub async fn clear_workspace(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    sqlx::query("DELETE FROM tracks").execute(&s.pool).await?;
    sqlx::query("DELETE FROM jobs").execute(&s.pool).await?;
    sqlx::query("DELETE FROM provider_cache")
        .execute(&s.pool)
        .await?;
    s.previews.write().await.clear();
    *s.workflow.write().await = Workflow {
        phase: "idle".into(),
        message: "Ready to scan".into(),
        ..Default::default()
    };
    Ok(Json(serde_json::json!({"cleared":true})))
}
pub async fn workspace(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    let mut workflow = s.workflow.read().await.clone();
    let matched: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tracks WHERE selected_candidate_id IS NOT NULL")
            .fetch_one(&s.pool)
            .await?;
    workflow.matched = matched as usize;
    if workflow.phase == "idle" && matched > 0 {
        workflow.phase = "preview".into();
        workflow.message = "Restored matched preview".into();
    }
    Ok(Json(serde_json::to_value(workflow)?))
}
pub async fn reset_settings_section(
    State(s): State<Arc<AppState>>,
    Path(section): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut cfg = s.config.read().await.clone();
    let defaults = Config::default();
    match section.as_str() {
        "matching" => {
            cfg.automation_mode = defaults.automation_mode;
            cfg.confidence_threshold = defaults.confidence_threshold;
            cfg.track_attempts = defaults.track_attempts;
            cfg.metadata_read_concurrency = defaults.metadata_read_concurrency;
            cfg.fingerprint_concurrency = defaults.fingerprint_concurrency;
            cfg.acoustid_concurrency = defaults.acoustid_concurrency;
            cfg.db_write_batch_size = defaults.db_write_batch_size;
        }
        "metadata" => {
            cfg.metadata_fields = defaults.metadata_fields;
            cfg.overwrite_existing_tags = defaults.overwrite_existing_tags;
            cfg.cover_art_enabled = defaults.cover_art_enabled;
        }
        "files" => {
            cfg.path_templates = defaults.path_templates;
            cfg.in_place = defaults.in_place;
            cfg.output_mode = defaults.output_mode;
            cfg.expert_mode = false;
        }
        _ => return Err(anyhow!("unknown settings section").into()),
    }
    crate::infrastructure::db::save_settings(&s.pool, &cfg).await?;
    *s.config.write().await = cfg;
    Ok(Json(serde_json::json!({"reset":section})))
}
pub async fn test_acoustid(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    let cfg = s.config.read().await.clone();
    if cfg.acoustid_api_key.is_empty() {
        return Err(anyhow!("AcoustID is not configured").into());
    }
    Ok(Json(
        serde_json::json!({"ok":true,"message":"AcoustID key is configured. It will be validated during matching."}),
    ))
}
pub async fn test_musicbrainz(
    State(s): State<Arc<AppState>>,
) -> ApiResult<Json<serde_json::Value>> {
    let cfg = s.config.read().await.clone();
    crate::infrastructure::providers::musicbrainz::test_connection(
        &s.client,
        &cfg.musicbrainz_user_agent,
    )
    .await?;
    Ok(Json(
        serde_json::json!({"ok":true,"message":"MusicBrainz connection is working"}),
    ))
}
pub async fn start_scan(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    if matches!(
        s.workflow.read().await.phase.as_str(),
        "scan" | "fetch" | "apply"
    ) {
        return Err(anyhow!("workflow is already running").into());
    }
    sqlx::query("DELETE FROM tracks").execute(&s.pool).await?;
    sqlx::query("DELETE FROM provider_cache")
        .execute(&s.pool)
        .await?;
    s.previews.write().await.clear();
    *s.workflow.write().await = Workflow {
        phase: "scan".into(),
        message: "Discovering music".into(),
        ..Default::default()
    };
    s.terminal(
        "info",
        "scan",
        None,
        "Starting new scan; cleared previous temporary workspace",
    )
    .await;
    let state = s.clone();
    tokio::spawn(async move {
        if let Err(error) = scan_pipeline::run(state.clone()).await {
            let mut w = state.workflow.write().await;
            w.phase = "failed".into();
            w.message = error.to_string();
            jobs::emit(&state, "workflow", Some("failed"), 0, 0, &error.to_string());
        }
    });
    Ok(Json(serde_json::json!({"started":true})))
}
pub async fn stop_scan(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    s.workflow.write().await.cancelled = true;
    Ok(Json(serde_json::json!({"stopping":true})))
}
pub async fn stop_apply(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    s.workflow.write().await.cancelled = true;
    Ok(Json(serde_json::json!({"stopping":true})))
}
pub async fn list_jobs(State(s): State<Arc<AppState>>) -> ApiResult<Json<Vec<serde_json::Value>>> {
    Ok(Json(vec![serde_json::to_value(
        s.workflow.read().await.clone(),
    )?]))
}
pub async fn get_job(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let _ = id;
    Ok(Json(serde_json::to_value(s.workflow.read().await.clone())?))
}
pub async fn list_tracks(
    State(s): State<Arc<AppState>>,
    Query(q): Query<TrackQuery>,
) -> ApiResult<Json<TrackPage>> {
    let page = q.page.unwrap_or(1).max(1);
    let size = q.page_size.unwrap_or(100).clamp(20, 200);
    let status = q.status.unwrap_or_default();
    let search = format!("%{}%", q.search.unwrap_or_default());
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM tracks WHERE (?='' OR stage=?) AND (?='%%' OR filename LIKE ? OR current_title LIKE ? OR current_artist LIKE ?)")
        .bind(&status).bind(&status).bind(&search).bind(&search).bind(&search).bind(&search).fetch_one(&s.pool).await?;
    let tracks: Vec<Track> = sqlx::query_as("SELECT id,path,output_path,filename,format,duration,current_title,current_artist,current_album,current_album_artist,current_track_number,selected_candidate_id,status,error,is_missing,stage,stage_message,retry_count,next_retry_at FROM tracks WHERE (?='' OR stage=?) AND (?='%%' OR filename LIKE ? OR current_title LIKE ? OR current_artist LIKE ?) ORDER BY path LIMIT ? OFFSET ?")
        .bind(&status).bind(&status).bind(&search).bind(&search).bind(&search).bind(&search).bind(size).bind((page-1)*size).fetch_all(&s.pool).await?;
    let mut result = Vec::with_capacity(tracks.len());
    for track in tracks {
        let candidates = fetch_candidates(&s.pool, track.id).await?;
        result.push(WorkspaceTrack { track, candidates });
    }
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT stage,count(*) FROM tracks GROUP BY stage")
            .fetch_all(&s.pool)
            .await?;
    Ok(Json(TrackPage {
        items: result,
        total,
        counts: rows.into_iter().collect(),
    }))
}
pub async fn get_track(
    State(s): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Track>> {
    Ok(Json(sqlx::query_as("SELECT id,path,output_path,filename,format,duration,current_title,current_artist,current_album,current_album_artist,current_track_number,selected_candidate_id,status,error,is_missing,stage,stage_message,retry_count,next_retry_at FROM tracks WHERE id=?").bind(id).fetch_one(&s.pool).await?))
}
pub async fn current_artwork(
    State(s): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    let path: String = sqlx::query_scalar("SELECT path FROM tracks WHERE id=?")
        .bind(id)
        .fetch_one(&s.pool)
        .await?;
    let artwork =
        tokio::task::spawn_blocking(move || crate::domain::audio::artwork(&PathBuf::from(path)))
            .await
            .map_err(|error| anyhow!("could not read artwork: {error}"))??;
    let Some(artwork) = artwork else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    Ok(image_response(artwork.mime, artwork.data))
}
pub async fn proposed_artwork(
    State(s): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> ApiResult<Response> {
    let cover_url: Option<String> =
        sqlx::query_scalar("SELECT cover_url FROM candidates WHERE id=?")
            .bind(id)
            .fetch_optional(&s.pool)
            .await?
            .flatten();
    let Some(url) = cover_url else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let cache_dir = PathBuf::from(&s.config.read().await.db_path)
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/cache"))
        .join("artwork");
    tokio::fs::create_dir_all(&cache_dir).await?;
    let cache_path = cache_dir.join(format!("candidate-{id}.img"));
    let mime_path = cache_dir.join(format!("candidate-{id}.mime"));
    if let Ok(data) = tokio::fs::read(&cache_path).await {
        let mime = tokio::fs::read_to_string(&mime_path)
            .await
            .unwrap_or_else(|_| "image/jpeg".into());
        return Ok(image_response(mime, data));
    }
    let response = s.client.get(url).send().await?.error_for_status()?;
    let mime = response
        .headers()
        .get(header::CONTENT_TYPE)
        .and_then(|value| value.to_str().ok())
        .unwrap_or("image/jpeg")
        .to_owned();
    let data = response.bytes().await?.to_vec();
    tokio::fs::write(&cache_path, &data).await?;
    tokio::fs::write(&mime_path, &mime).await?;
    Ok(image_response(mime, data))
}
pub(super) fn image_response(mime: impl AsRef<str>, data: Vec<u8>) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, mime.as_ref())
        .header(header::CACHE_CONTROL, "public, max-age=86400")
        .body(Body::from(data))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
pub async fn candidates(
    State(s): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<CandidateRow>>> {
    Ok(Json(fetch_candidates(&s.pool, id).await?))
}
pub async fn select_candidate(
    State(s): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<SelectRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    sqlx::query("UPDATE tracks SET selected_candidate_id=?,status=CASE WHEN ? IS NULL THEN 'needs_review' ELSE 'selected' END WHERE id=?").bind(body.candidate_id).bind(body.candidate_id).bind(id).execute(&s.pool).await?;
    s.previews.write().await.clear();
    Ok(Json(serde_json::json!({"selected":true})))
}
pub async fn edit_candidate(
    State(s): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(v): Json<CandidateEdit>,
) -> ApiResult<Json<serde_json::Value>> {
    sqlx::query("UPDATE candidates SET title=?,artist=?,album=?,album_artist=?,track_number=?,track_total=?,disc_number=?,disc_total=?,year=?,genre=?,composer=?,label=?,isrc=?,provider='manual' WHERE id=?")
        .bind(v.title).bind(v.artist).bind(v.album).bind(v.album_artist).bind(v.track_number).bind(v.track_total).bind(v.disc_number).bind(v.disc_total).bind(v.year).bind(v.genre).bind(v.composer).bind(v.label).bind(v.isrc).bind(id).execute(&s.pool).await?;
    s.previews.write().await.clear();
    Ok(Json(serde_json::json!({"saved":true})))
}
pub async fn retry_track(
    State(s): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> ApiResult<Json<serde_json::Value>> {
    sqlx::query(
        "UPDATE tracks SET file_mtime=-1,stage='discovered',status='new',error=NULL WHERE id=?",
    )
    .bind(id)
    .execute(&s.pool)
    .await?;
    start_scan(State(s)).await
}
pub async fn retry_failed(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    sqlx::query("UPDATE tracks SET file_mtime=-1,stage='discovered',status='new',error=NULL WHERE stage='failed'").execute(&s.pool).await?;
    start_scan(State(s)).await
}
pub async fn skip_review(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    sqlx::query("UPDATE tracks SET selected_candidate_id=NULL,status='skipped',stage='skipped' WHERE stage='review'").execute(&s.pool).await?;
    Ok(Json(serde_json::json!({"skipped":true})))
}
pub async fn template_preview(
    State(s): State<Arc<AppState>>,
    Json(body): Json<PreviewRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let base = s.config.read().await.clone();
    let mut cfg = body.settings.unwrap_or(base.clone());
    cfg.acoustid_api_key = base.acoustid_api_key;
    cfg.musicbrainz_user_agent = base.musicbrainz_user_agent;
    cfg.db_path = base.db_path;
    let sample_track = Track {
        id: 0,
        path: format!("{}/Song Title.mp3", cfg.input_dir),
        output_path: None,
        filename: "Song Title.mp3".into(),
        format: Some("mp3".into()),
        duration: Some(212.0),
        current_title: Some("Wrong Title".into()),
        current_artist: Some("Wrong Artist".into()),
        current_album: Some("Wrong Album".into()),
        current_album_artist: Some("Wrong Album Artist".into()),
        current_track_number: Some(9),
        selected_candidate_id: Some(0),
        status: "sample".into(),
        error: None,
        is_missing: false,
        stage: "ready".into(),
        stage_message: None,
        retry_count: 0,
        next_retry_at: None,
    };
    let sample_candidate = Candidate {
        id: Some(0),
        title: "Song Title".into(),
        artist: "Artist".into(),
        album: Some("Album".into()),
        album_artist: Some("Album Artist".into()),
        track_number: Some(1),
        track_total: Some(10),
        disc_number: Some(1),
        disc_total: Some(1),
        year: Some("2024".into()),
        genre: Some("Rock".into()),
        composer: Some("Composer".into()),
        label: Some("Label".into()),
        isrc: Some("USRC17607839".into()),
        cover_url: None,
        recording_id: Some("sample-recording".into()),
        release_id: Some("sample-release".into()),
        artist_id: Some("sample-artist".into()),
        album_artist_id: Some("sample-album-artist".into()),
        score: 96.0,
        raw_json: "{}".into(),
    };
    let (track, candidate) = if let Some(id) = body.track_id {
        selected(&s.pool, id).await?
    } else {
        (sample_track, sample_candidate)
    };
    let mut previews = vec![
        (
            "Output template",
            cfg.path_templates.default_template.clone(),
        ),
        (
            "Compilation template",
            cfg.path_templates.compilation_template.clone(),
        ),
        (
            "In-place filename template",
            cfg.in_place.filename_template.clone(),
        ),
    ];
    if let Some(template) = body.template {
        previews = vec![("Custom template", template)];
    }
    let results: Vec<PathPreviewResult> = previews
        .into_iter()
        .map(|(label, template)| path_preview_result(&cfg, &track, &candidate, label, template))
        .collect();
    Ok(Json(serde_json::json!({
        "examples": results,
        "sample": {
            "artist":"Artist",
            "albumartist":"Album Artist",
            "album":"Album",
            "title":"Song Title",
            "track":"01",
            "year":"2024",
            "ext":"mp3"
        }
    })))
}
pub async fn apply_preview(
    State(s): State<Arc<AppState>>,
    Json(_): Json<serde_json::Value>,
) -> ApiResult<Json<serde_json::Value>> {
    let tracks:Vec<Track>=sqlx::query_as("SELECT id,path,output_path,filename,format,duration,current_title,current_artist,current_album,current_album_artist,current_track_number,selected_candidate_id,status,error,is_missing,stage,stage_message,retry_count,next_retry_at FROM tracks WHERE selected_candidate_id IS NOT NULL AND is_missing=0").fetch_all(&s.pool).await?;
    let cfg = s.config.read().await.clone();
    let selected = load_selected(&s.pool, tracks).await?;
    let duplicates = duplicate_actions(&selected);
    let duplicate_skipped = duplicates
        .values()
        .filter(|action| action.duplicate_action == "skip_duplicate")
        .count();
    let mut items = Vec::new();
    for (track, candidate) in selected {
        let dup = duplicates.get(&track.id).cloned().unwrap_or_default();
        let dest = destination(&cfg, &track, &candidate, None)?;
        let mut warnings = vec![];
        if matches!(track.format.as_deref(), Some("wav" | "aiff" | "aif")) {
            warnings.push("Tag writing will be skipped: conditional/unsafe format".into());
        }
        if dup.duplicate_action == "skip_duplicate" {
            warnings.push(dup.duplicate_reason.clone().unwrap_or_else(|| {
                "Duplicate of a stronger matched file; it will not be written".into()
            }));
        }
        let candidate_id = candidate
            .id
            .ok_or_else(|| anyhow!("selected candidate is missing a database id"))?;
        items.push(PreviewItem {
            track_id: track.id,
            candidate_id,
            filename: track.filename.clone(),
            current_path: track.path.clone(),
            destination_path: dest,
            action: if cfg.output_mode == "copy" {
                "copy + write tags".into()
            } else {
                "write tags".into()
            },
            warnings,
            duplicate_group_id: dup.duplicate_group_id,
            duplicate_action: dup.duplicate_action,
            duplicate_reason: dup.duplicate_reason,
            kept_track_id: dup.kept_track_id,
            old: old_summary(&track),
            new: new_summary(&candidate, &track),
            cover_url: candidate.cover_url.clone(),
            current_cover_url: Some(format!("/api/artwork/current/{}", track.id)),
            proposed_cover_url: candidate
                .cover_url
                .as_ref()
                .map(|_| format!("/api/artwork/proposed/{candidate_id}")),
            confidence: candidate.score,
            artwork_action: if cfg.cover_art_enabled && candidate.cover_url.is_some() {
                "download + embed cover art".into()
            } else {
                "no artwork change".into()
            },
        });
    }
    let token = Uuid::new_v4().to_string();
    let apply_items: Vec<PreviewItem> = items
        .iter()
        .filter(|item| item.duplicate_action != "skip_duplicate")
        .cloned()
        .collect();
    s.previews.write().await.insert(token.clone(), apply_items);
    Ok(Json(serde_json::json!({
        "preview_token":token,
        "items":items,
        "summary":{
            "write_count": items.iter().filter(|item| item.duplicate_action != "skip_duplicate").count(),
            "duplicate_skipped": duplicate_skipped
        }
    })))
}
pub async fn start_apply(
    State(s): State<Arc<AppState>>,
    Json(body): Json<ApplyRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let items = s
        .previews
        .write()
        .await
        .remove(&body.preview_token)
        .ok_or_else(|| anyhow!("a current successful dry-run preview is required"))?;
    let id = Uuid::new_v4().to_string();
    {
        let mut w = s.workflow.write().await;
        w.phase = "apply".into();
        w.message = "Applying matched metadata".into();
        w.cancelled = false;
    }
    let state = s.clone();
    let job = id.clone();
    tokio::spawn(async move {
        let result = apply(state.clone(), job.clone(), items).await;
        let mut w = state.workflow.write().await;
        w.phase = if result.is_ok() {
            "finish".into()
        } else {
            "failed".into()
        };
        w.message = result
            .err()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "Apply complete".into());
    });
    Ok(Json(serde_json::json!({"job_id":id})))
}
pub async fn apply(s: Arc<AppState>, job: String, items: Vec<PreviewItem>) -> Result<()> {
    let total = items.len() as i64;
    let cfg = s.config.read().await.clone();
    for (i, item) in items.into_iter().enumerate() {
        if s.cancelled(&job).await {
            break;
        }
        let (_, candidate) = selected(&s.pool, item.track_id).await?;
        let artwork = if cfg.cover_art_enabled {
            if let Some(url) = &candidate.cover_url {
                crate::infrastructure::providers::cover_art_archive::fetch(&s.client, url)
                    .await
                    .ok()
            } else {
                None
            }
        } else {
            None
        };
        let src = PathBuf::from(&item.current_path);
        let dest = PathBuf::from(&item.destination_path);
        let target = if cfg.output_mode == "copy" {
            if let Some(p) = dest.parent() {
                tokio::fs::create_dir_all(p).await?;
            }
            tokio::fs::copy(&src, &dest).await?;
            dest.clone()
        } else {
            src
        };
        let write_target = target.clone();
        let original_mtime = if cfg.in_place.preserve_mtime {
            std::fs::metadata(&write_target)
                .ok()
                .map(|m| filetime::FileTime::from_last_modification_time(&m))
        } else {
            None
        };
        let result = tokio::task::spawn_blocking({
            let cfg = cfg.clone();
            move || {
                tag_writer::write(&write_target, &candidate, &cfg, artwork)?;
                if let Some(mtime) = original_mtime {
                    filetime::set_file_mtime(&write_target, mtime)?;
                }
                Ok::<_, anyhow::Error>(())
            }
        })
        .await?;
        let (status, error) = match result {
            Ok(_) => {
                if cfg.output_mode == "in_place" && target != dest {
                    if let Some(parent) = dest.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    tokio::fs::rename(&target, &dest).await?;
                    sqlx::query("UPDATE tracks SET path=?,filename=? WHERE id=?")
                        .bind(dest.to_string_lossy())
                        .bind(dest.file_name().and_then(|v| v.to_str()).unwrap_or("audio"))
                        .bind(item.track_id)
                        .execute(&s.pool)
                        .await?;
                }
                ("applied", None)
            }
            Err(e) => ("failed", Some(e.to_string())),
        };
        if status == "failed" && cfg.output_mode == "copy" {
            let _ = tokio::fs::remove_file(&target).await;
        }
        let final_path = if status == "applied" && cfg.output_mode == "in_place" {
            &dest
        } else {
            &target
        };
        sqlx::query(
            "UPDATE tracks SET output_path=?,status=?,error=?,last_applied_at=? WHERE id=?",
        )
        .bind(final_path.to_string_lossy())
        .bind(status)
        .bind(error)
        .bind(Utc::now().to_rfc3339())
        .bind(item.track_id)
        .execute(&s.pool)
        .await?;
        {
            let mut w = s.workflow.write().await;
            w.current = i + 1;
            w.total = total as usize;
            w.current_file = Some(item.current_path.clone());
        }
        jobs::emit(&s, "workflow", Some("apply"), i as i64 + 1, total, status);
        if status == "applied" {
            sqlx::query("DELETE FROM tracks WHERE id=?")
                .bind(item.track_id)
                .execute(&s.pool)
                .await?;
        }
    }
    Ok(())
}
fn destination(
    cfg: &Config,
    track: &Track,
    c: &Candidate,
    template: Option<&str>,
) -> Result<String> {
    if cfg.output_mode == "in_place" && !cfg.in_place.rename_files && !cfg.in_place.rename_folders {
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
    let chosen = template.unwrap_or(if compilation && cfg.output_mode == "copy" {
        &cfg.path_templates.compilation_template
    } else if cfg.output_mode == "in_place" && !cfg.in_place.rename_folders {
        &cfg.in_place.filename_template
    } else {
        &cfg.path_templates.default_template
    });
    let relative = path_templates::render(chosen, &values, &cfg.path_templates)?;
    let root = if cfg.output_mode == "copy" {
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
        &cfg.path_templates.collision_strategy,
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
struct DuplicateAction {
    duplicate_group_id: Option<String>,
    duplicate_action: String,
    duplicate_reason: Option<String>,
    kept_track_id: Option<i64>,
}

async fn load_selected(pool: &SqlitePool, tracks: Vec<Track>) -> Result<Vec<(Track, Candidate)>> {
    let mut out = Vec::with_capacity(tracks.len());
    for track in tracks {
        let (_, candidate) = selected(pool, track.id).await?;
        out.push((track, candidate));
    }
    Ok(out)
}

fn duplicate_actions(selected: &[(Track, Candidate)]) -> HashMap<i64, DuplicateAction> {
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
                DuplicateAction {
                    duplicate_group_id: Some(key.clone()),
                    duplicate_action: if track.id == keep_id {
                        "keep".into()
                    } else {
                        "skip_duplicate".into()
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

async fn selected(pool: &SqlitePool, id: i64) -> Result<(Track, Candidate)> {
    let track:Track=sqlx::query_as("SELECT id,path,output_path,filename,format,duration,current_title,current_artist,current_album,current_album_artist,current_track_number,selected_candidate_id,status,error,is_missing,stage,stage_message,retry_count,next_retry_at FROM tracks WHERE id=?").bind(id).fetch_one(pool).await?;
    let cid = track
        .selected_candidate_id
        .ok_or_else(|| anyhow!("track has no selected candidate"))?;
    let row: CandidateRow = sqlx::query_as("SELECT * FROM candidates WHERE id=?")
        .bind(cid)
        .fetch_one(pool)
        .await?;
    Ok((track, row.value()))
}
async fn fetch_candidates(pool: &SqlitePool, id: i64) -> Result<Vec<CandidateRow>> {
    Ok(
        sqlx::query_as("SELECT * FROM candidates WHERE track_id=? ORDER BY score DESC")
            .bind(id)
            .fetch_all(pool)
            .await?,
    )
}
pub async fn events(
    State(s): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = std::result::Result<Event, Infallible>>> {
    let mut rx = s.events.subscribe();
    Sse::new(
        async_stream::stream! {while let Ok(value)=rx.recv().await{yield Ok(Event::default().json_data(value).unwrap());}},
    )
}
