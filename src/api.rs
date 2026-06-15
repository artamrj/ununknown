use crate::{
    config::Config,
    fs_scan, jobs,
    path_templates::{self, TemplateValues},
    providers::Candidate,
    tag_writer,
};
use anyhow::{Result, anyhow};
use axum::{
    Json, Router,
    extract::{Path, State},
    http::StatusCode,
    response::{IntoResponse, Response, Sse, sse::Event},
    routing::{get, post},
};
use chrono::Utc;
use futures::Stream;
use serde::{Deserialize, Serialize};
use sqlx::{FromRow, SqlitePool};
use std::{
    collections::{HashMap, HashSet},
    convert::Infallible,
    path::PathBuf,
    sync::Arc,
};
use tokio::sync::{RwLock, broadcast};
use uuid::Uuid;

pub struct AppState {
    pub config: RwLock<Config>,
    pub pool: SqlitePool,
    pub client: reqwest::Client,
    pub events: broadcast::Sender<jobs::Event>,
    cancelled: RwLock<HashSet<String>>,
    previews: RwLock<HashMap<String, Vec<PreviewItem>>>,
}
impl AppState {
    pub fn new(config: Config, pool: SqlitePool) -> Self {
        let (events, _) = broadcast::channel(256);
        Self {
            config: RwLock::new(config),
            pool,
            client: reqwest::Client::new(),
            events,
            cancelled: Default::default(),
            previews: Default::default(),
        }
    }
    pub async fn cancelled(&self, id: &str) -> bool {
        self.cancelled.read().await.contains(id)
    }
}

#[derive(Debug)]
struct ApiError(anyhow::Error);
impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(value: E) -> Self {
        Self(value.into())
    }
}
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        (
            StatusCode::BAD_REQUEST,
            Json(serde_json::json!({"error":self.0.to_string()})),
        )
            .into_response()
    }
}
type ApiResult<T> = std::result::Result<T, ApiError>;

#[derive(Serialize, FromRow)]
struct Track {
    id: i64,
    path: String,
    output_path: Option<String>,
    filename: String,
    format: Option<String>,
    duration: Option<f64>,
    current_title: Option<String>,
    current_artist: Option<String>,
    current_album: Option<String>,
    selected_candidate_id: Option<i64>,
    status: String,
    error: Option<String>,
    is_missing: bool,
}
#[derive(Serialize, FromRow)]
struct CandidateRow {
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
struct PreviewItem {
    track_id: i64,
    current_path: String,
    destination_path: String,
    action: String,
    warnings: Vec<String>,
}
#[derive(Deserialize)]
struct SelectRequest {
    candidate_id: Option<i64>,
}
#[derive(Deserialize)]
struct PreviewRequest {
    template: Option<String>,
    track_id: Option<i64>,
}
#[derive(Deserialize)]
struct ApplyRequest {
    preview_token: String,
}

pub fn router() -> Router<Arc<AppState>> {
    Router::new()
        .route(
            "/health",
            get(|| async { Json(serde_json::json!({"status":"ok"})) }),
        )
        .route("/settings", get(settings).put(update_settings))
        .route("/scan/start", post(start_scan))
        .route("/scan/stop", post(stop_scan))
        .route("/jobs", get(list_jobs))
        .route("/jobs/{id}", get(get_job))
        .route("/tracks", get(list_tracks))
        .route("/tracks/{id}", get(get_track))
        .route("/tracks/{id}/candidates", get(candidates))
        .route("/tracks/{id}/select-candidate", post(select_candidate))
        .route("/path-template/preview", post(template_preview))
        .route("/apply/preview", post(apply_preview))
        .route("/apply/start", post(start_apply))
        .route("/apply/stop", post(stop_apply))
        .route("/events", get(events))
}
async fn settings(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::to_value(s.config.read().await.public()).unwrap())
}
async fn update_settings(
    State(s): State<Arc<AppState>>,
    Json(mut cfg): Json<Config>,
) -> ApiResult<Json<serde_json::Value>> {
    cfg.acoustid_api_key = s.config.read().await.acoustid_api_key.clone();
    cfg.db_path = s.config.read().await.db_path.clone();
    *s.config.write().await = cfg;
    s.previews.write().await.clear();
    Ok(Json(serde_json::json!({"saved":true})))
}
async fn start_scan(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    s.previews.write().await.clear();
    let id = jobs::create(&s, "scan").await?;
    let state = s.clone();
    let job = id.clone();
    tokio::spawn(async move {
        let result = fs_scan::run(state.clone(), job.clone()).await;
        jobs::finish(
            &state,
            "scan",
            &job,
            result.as_ref().err().map(|e| e.to_string()).as_deref(),
        )
        .await;
    });
    Ok(Json(serde_json::json!({"job_id":id})))
}
async fn stop_scan(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    cancel_kind(&s, "scan").await?;
    Ok(Json(serde_json::json!({"stopping":true})))
}
async fn stop_apply(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    cancel_kind(&s, "apply").await?;
    Ok(Json(serde_json::json!({"stopping":true})))
}
async fn cancel_kind(s: &Arc<AppState>, kind: &str) -> Result<()> {
    let ids: Vec<String> =
        sqlx::query_scalar("SELECT id FROM jobs WHERE kind=? AND status='running'")
            .bind(kind)
            .fetch_all(&s.pool)
            .await?;
    s.cancelled.write().await.extend(ids);
    Ok(())
}
async fn list_jobs(State(s): State<Arc<AppState>>) -> ApiResult<Json<Vec<serde_json::Value>>> {
    Ok(Json(sqlx::query_scalar("SELECT json_object('id',id,'kind',kind,'status',status,'progress_current',progress_current,'progress_total',progress_total,'error',error) FROM jobs ORDER BY created_at DESC").fetch_all(&s.pool).await?.into_iter().filter_map(|v:String|serde_json::from_str(&v).ok()).collect()))
}
async fn get_job(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let v:String=sqlx::query_scalar("SELECT json_object('id',id,'kind',kind,'status',status,'progress_current',progress_current,'progress_total',progress_total,'error',error) FROM jobs WHERE id=?").bind(id).fetch_one(&s.pool).await?;
    Ok(Json(serde_json::from_str(&v)?))
}
async fn list_tracks(State(s): State<Arc<AppState>>) -> ApiResult<Json<Vec<Track>>> {
    Ok(Json(sqlx::query_as("SELECT id,path,output_path,filename,format,duration,current_title,current_artist,current_album,selected_candidate_id,status,error,is_missing FROM tracks ORDER BY path").fetch_all(&s.pool).await?))
}
async fn get_track(State(s): State<Arc<AppState>>, Path(id): Path<i64>) -> ApiResult<Json<Track>> {
    Ok(Json(sqlx::query_as("SELECT id,path,output_path,filename,format,duration,current_title,current_artist,current_album,selected_candidate_id,status,error,is_missing FROM tracks WHERE id=?").bind(id).fetch_one(&s.pool).await?))
}
async fn candidates(
    State(s): State<Arc<AppState>>,
    Path(id): Path<i64>,
) -> ApiResult<Json<Vec<CandidateRow>>> {
    Ok(Json(fetch_candidates(&s.pool, id).await?))
}
async fn select_candidate(
    State(s): State<Arc<AppState>>,
    Path(id): Path<i64>,
    Json(body): Json<SelectRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    sqlx::query("UPDATE tracks SET selected_candidate_id=?,status=CASE WHEN ? IS NULL THEN 'needs_review' ELSE 'selected' END WHERE id=?").bind(body.candidate_id).bind(body.candidate_id).bind(id).execute(&s.pool).await?;
    s.previews.write().await.clear();
    Ok(Json(serde_json::json!({"selected":true})))
}
async fn template_preview(
    State(s): State<Arc<AppState>>,
    Json(body): Json<PreviewRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let id = body.track_id.ok_or_else(|| anyhow!("track_id required"))?;
    let (track, candidate) = selected(&s.pool, id).await?;
    let cfg = s.config.read().await;
    let path = destination(&cfg, &track, &candidate, body.template.as_deref())?;
    Ok(Json(serde_json::json!({"path":path})))
}
async fn apply_preview(
    State(s): State<Arc<AppState>>,
    Json(_): Json<serde_json::Value>,
) -> ApiResult<Json<serde_json::Value>> {
    let tracks:Vec<Track>=sqlx::query_as("SELECT id,path,output_path,filename,format,duration,current_title,current_artist,current_album,selected_candidate_id,status,error,is_missing FROM tracks WHERE selected_candidate_id IS NOT NULL AND is_missing=0").fetch_all(&s.pool).await?;
    let cfg = s.config.read().await.clone();
    let mut items = Vec::new();
    for track in tracks {
        let (_, candidate) = selected(&s.pool, track.id).await?;
        let dest = destination(&cfg, &track, &candidate, None)?;
        let mut warnings = vec![];
        if matches!(track.format.as_deref(), Some("wav" | "aiff" | "aif")) {
            warnings.push("Tag writing will be skipped: conditional/unsafe format".into());
        }
        items.push(PreviewItem {
            track_id: track.id,
            current_path: track.path.clone(),
            destination_path: dest,
            action: if cfg.output_mode == "copy" {
                "copy + write tags".into()
            } else {
                "write tags".into()
            },
            warnings,
        });
    }
    let token = Uuid::new_v4().to_string();
    s.previews
        .write()
        .await
        .insert(token.clone(), items.clone());
    Ok(Json(
        serde_json::json!({"preview_token":token,"items":items}),
    ))
}
async fn start_apply(
    State(s): State<Arc<AppState>>,
    Json(body): Json<ApplyRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let items = s
        .previews
        .write()
        .await
        .remove(&body.preview_token)
        .ok_or_else(|| anyhow!("a current successful dry-run preview is required"))?;
    let id = jobs::create(&s, "apply").await?;
    let state = s.clone();
    let job = id.clone();
    tokio::spawn(async move {
        let result = apply(state.clone(), job.clone(), items).await;
        jobs::finish(
            &state,
            "apply",
            &job,
            result.as_ref().err().map(|e| e.to_string()).as_deref(),
        )
        .await;
    });
    Ok(Json(serde_json::json!({"job_id":id})))
}
async fn apply(s: Arc<AppState>, job: String, items: Vec<PreviewItem>) -> Result<()> {
    let total = items.len() as i64;
    let cfg = s.config.read().await.clone();
    for (i, item) in items.into_iter().enumerate() {
        if s.cancelled(&job).await {
            break;
        }
        let (_, candidate) = selected(&s.pool, item.track_id).await?;
        let artwork = if cfg.cover_art_enabled {
            if let Some(url) = &candidate.cover_url {
                crate::providers::cover_art_archive::fetch(&s.client, url)
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
        let result = tokio::task::spawn_blocking({
            let cfg = cfg.clone();
            move || tag_writer::write(&write_target, &candidate, &cfg, artwork)
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
        jobs::progress(&s, "apply", &job, i as i64 + 1, total, status).await;
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
    let chosen = template.unwrap_or(
        if cfg.output_mode == "in_place" && !cfg.in_place.rename_folders {
            &cfg.in_place.filename_template
        } else {
            &cfg.path_templates.default_template
        },
    );
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
async fn selected(pool: &SqlitePool, id: i64) -> Result<(Track, Candidate)> {
    let track:Track=sqlx::query_as("SELECT id,path,output_path,filename,format,duration,current_title,current_artist,current_album,selected_candidate_id,status,error,is_missing FROM tracks WHERE id=?").bind(id).fetch_one(pool).await?;
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
async fn events(
    State(s): State<Arc<AppState>>,
) -> Sse<impl Stream<Item = std::result::Result<Event, Infallible>>> {
    let mut rx = s.events.subscribe();
    Sse::new(
        async_stream::stream! {while let Ok(value)=rx.recv().await{yield Ok(Event::default().json_data(value).unwrap());}},
    )
}
