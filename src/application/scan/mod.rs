//! Scan workflow drivers: run/run_automatic/retry/run_files plus the shared
//! types the rest of the scan pipeline operates on.

pub(super) use crate::{
    app::{ActivityLogEntry, AppState},
    application::input_dedup::{self, RecordingEvidence},
    domain::audio,
    domain::matcher::{
        artist_similarity, normalize_match_key, text_close, text_similarity, title_similarity,
    },
    infrastructure::{fingerprint_cache, media::fingerprint},
    types::WorkflowPhase,
};
pub(super) use crate::infrastructure::providers as infra_providers;
pub(super) use anyhow::{Context, Result, anyhow};
pub(super) use chrono::Utc;
pub(super) use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};
pub(super) use tokio::{
    sync::{Mutex, Semaphore, mpsc, oneshot},
    task::JoinSet,
};
pub(super) use walkdir::WalkDir;

mod persist;
mod process;
mod providers;
mod score;

pub(super) use persist::*;
pub(super) use process::*;
pub(super) use providers::*;
pub(super) use score::*;

pub async fn run(state: Arc<AppState>) -> Result<()> {
    let cfg = state.config.read().await.clone();
    state
        .set_workflow(WorkflowPhase::Scan, "scan", "Discovering music", 0, 0, None)
        .await;
    state
        .log(
            "info",
            "scan",
            None,
            "Walking input folder for supported audio files",
        )
        .await;
    let (files, walk_errors) = discover_files_in_background(cfg.input_dir.clone()).await?;
    let total = files.len();
    state
        .log(
            "info",
            "scan",
            None,
            &format!("Discovered {total} supported audio files"),
        )
        .await;
    for error in walk_errors {
        state
            .log_entry(
                ActivityLogEntry::new("error", "scan", "Input folder walk error").error_text(error),
            )
            .await;
    }
    if total == 0 {
        state
            .log_entry(
                ActivityLogEntry::new("warn", "scan", "No supported audio files found")
                    .detail(format!("Input folder scanned: {}", cfg.input_dir)),
            )
            .await;
    }
    let snapshot_files = files.clone();
    let cancelled = run_files(
        state.clone(),
        files,
        "Starting staged matching",
        "Matching complete",
    )
    .await?;
    if !cancelled {
        remember_scanned_files(&state.pool, &snapshot_files).await?;
    }
    Ok(())
}

pub async fn run_automatic(state: Arc<AppState>) -> Result<usize> {
    if !state
        .try_claim_workflow(
            WorkflowPhase::Scan,
            "Automatic scan: checking input folder",
            true,
        )
        .await
    {
        return Ok(0);
    }
    if state.workflow_cancelled().await || state.frontend_active_until().await.is_some() {
        state
            .finish_workflow(
                WorkflowPhase::Idle,
                "idle",
                "Automatic scan paused while the web app is open",
            )
            .await;
        return Ok(0);
    }
    let cfg = state.config.read().await.clone();
    let (files, walk_errors) = discover_files_in_background(cfg.input_dir).await?;
    for error in walk_errors {
        state
            .log_entry(
                ActivityLogEntry::new("error", "automatic_scan", "Input folder walk error")
                    .error_text(error),
            )
            .await;
    }
    let pending = automatic_pending_files(&state.pool, files).await?;
    if pending.is_empty() || state.frontend_active_until().await.is_some() {
        state
            .finish_workflow(
                WorkflowPhase::Idle,
                "idle",
                "Automatic scan: no new or changed music",
            )
            .await;
        return Ok(0);
    }
    if state.frontend_active_until().await.is_some() {
        state
            .finish_workflow(
                WorkflowPhase::Idle,
                "idle",
                "Automatic scan paused while the web app is open",
            )
            .await;
        return Ok(0);
    }
    let snapshot_files = pending.clone();
    let cancelled = run_files(
        state.clone(),
        pending,
        "Automatic scan: matching new or changed music",
        "Automatic scan complete",
    )
    .await?;
    if !cancelled {
        remember_scanned_files(&state.pool, &snapshot_files).await?;
    }
    Ok(if cancelled { 0 } else { snapshot_files.len() })
}

pub async fn retry_files(state: Arc<AppState>, files: Vec<PathBuf>) -> Result<()> {
    let snapshot_files = files.clone();
    let cancelled = run_files(
        state.clone(),
        files,
        "Checking files with issues",
        "Issue check complete",
    )
    .await?;
    if !cancelled {
        remember_scanned_files(&state.pool, &snapshot_files).await?;
    }
    Ok(())
}

fn discover_files(input_dir: &str) -> (Vec<PathBuf>, Vec<String>) {
    let mut walk_errors = Vec::new();
    let mut files: Vec<PathBuf> = WalkDir::new(input_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(|entry| match entry {
            Ok(entry) => Some(entry),
            Err(error) => {
                tracing::warn!("input folder walk error: {error:#}");
                walk_errors.push(format!("{error:#}"));
                None
            }
        })
        .filter(|e| e.file_type().is_file() && audio::is_supported(e.path()))
        .filter_map(|e| e.path().canonicalize().ok())
        .collect();
    files.sort();
    (files, walk_errors)
}

async fn discover_files_in_background(input_dir: String) -> Result<(Vec<PathBuf>, Vec<String>)> {
    tokio::task::spawn_blocking(move || discover_files(&input_dir))
        .await
        .context("music folder discovery task failed")
}

async fn automatic_pending_files(
    pool: &sqlx::SqlitePool,
    files: Vec<PathBuf>,
) -> Result<Vec<PathBuf>> {
    let rows: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT path,file_size,file_mtime_ns FROM automatic_scan_files ORDER BY path",
    )
    .fetch_all(pool)
    .await?;
    let mut known = rows.into_iter().peekable();
    let mut pending = Vec::new();
    for path in files {
        let Some((size, modified)) = file_snapshot(&path).await else {
            continue;
        };
        let text = path.to_string_lossy().into_owned();
        while known
            .peek()
            .is_some_and(|(known_path, _, _)| known_path < &text)
        {
            known.next();
        }
        let unchanged = known
            .peek()
            .is_some_and(|(known_path, known_size, known_modified)| {
                known_path == &text && *known_size == size && *known_modified == modified
            });
        if !unchanged {
            pending.push(path);
        }
    }
    Ok(pending)
}

async fn remember_scanned_files(pool: &sqlx::SqlitePool, files: &[PathBuf]) -> Result<()> {
    let mut transaction = pool.begin().await?;
    for path in files {
        let Some((size, modified)) = file_snapshot(path).await else {
            continue;
        };
        sqlx::query(
            "INSERT INTO automatic_scan_files(path,file_size,file_mtime_ns,checked_at)
             VALUES(?,?,?,?)
             ON CONFLICT(path) DO UPDATE SET
               file_size=excluded.file_size,
               file_mtime_ns=excluded.file_mtime_ns,
               checked_at=excluded.checked_at",
        )
        .bind(path.to_string_lossy().as_ref())
        .bind(size)
        .bind(modified)
        .bind(Utc::now().to_rfc3339())
        .execute(&mut *transaction)
        .await?;
    }
    transaction.commit().await?;
    Ok(())
}

async fn file_snapshot(path: &Path) -> Option<(i64, i64)> {
    let metadata = tokio::fs::metadata(path).await.ok()?;
    if !metadata.is_file() {
        return None;
    }
    let size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);
    let modified = metadata.modified().unwrap_or(SystemTime::UNIX_EPOCH);
    let nanos = modified
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    Some((size, i64::try_from(nanos).unwrap_or(i64::MAX)))
}

async fn run_files(
    state: Arc<AppState>,
    mut files: Vec<PathBuf>,
    starting_message: &'static str,
    completion_message: &'static str,
) -> Result<bool> {
    let cfg = state.config.read().await.clone();
    files.sort();
    files.dedup();
    let total = files.len();
    state
        .set_workflow(
            WorkflowPhase::Fetch,
            "fetch",
            starting_message,
            0,
            total,
            None,
        )
        .await;

    let limits = Arc::new(PipelineLimits {
        metadata: Arc::new(Semaphore::new(cfg.scan_workers)),
        fingerprint: Arc::new(Semaphore::new(cfg.fingerprint_workers)),
        acoustid: Arc::new(Semaphore::new(cfg.lookup_workers)),
        // Two workers keep first-time recognition from serializing the whole
        // library while still avoiding the burst traffic that triggers Shazam
        // rate limits.
        songrec: Arc::new(Semaphore::new(2)),
        disabled_providers: Arc::new(Mutex::new(HashSet::new())),
    });
    state
        .set_workflow(
            WorkflowPhase::Fetch,
            "deduplicate",
            "Analyzing input recordings for duplicates",
            0,
            total,
            None,
        )
        .await;
    let jobs = prepare_input_jobs(&state, &limits, files).await?;
    if state.workflow_cancelled().await {
        return Ok(true);
    }
    state
        .set_workflow(
            WorkflowPhase::Fetch,
            "fetch",
            starting_message,
            0,
            total,
            None,
        )
        .await;
    let (persist_tx, persist_rx) = mpsc::channel(25);
    let writer = tokio::spawn(db_writer(state.clone(), persist_rx, 25));
    let scan_workers = cfg.scan_workers.max(1).min(total.max(1));
    let (file_tx, file_rx) = mpsc::channel::<FileJob>(scan_workers * 2);
    let file_rx = Arc::new(Mutex::new(file_rx));
    let mut tasks = JoinSet::new();
    for _ in 0..scan_workers {
        let state = state.clone();
        let limits = limits.clone();
        let persist_tx = persist_tx.clone();
        let file_rx = file_rx.clone();
        tasks.spawn(async move {
            loop {
                let job = {
                    let mut rx = file_rx.lock().await;
                    rx.recv().await
                };
                let Some(job) = job else {
                    break;
                };
                process_file(
                    state.clone(),
                    limits.clone(),
                    persist_tx.clone(),
                    job,
                    total,
                )
                .await;
            }
        });
    }
    for job in jobs {
        if state.workflow_cancelled().await {
            break;
        }
        file_tx.send(job).await?;
    }
    drop(file_tx);
    drop(persist_tx);

    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            tracing::warn!("scan worker task failed: {error:#}");
            state.increment_failed().await;
        }
        if state.workflow_cancelled().await {
            tasks.abort_all();
        }
    }
    let writer_result = writer.await?;
    writer_result?;
    let cancelled = state.workflow_cancelled().await;
    state
        .finish_workflow(
            if cancelled {
                WorkflowPhase::Idle
            } else {
                WorkflowPhase::Preview
            },
            if cancelled { "idle" } else { "preview" },
            if cancelled {
                "Scan stopped"
            } else {
                completion_message
            },
        )
        .await;
    Ok(cancelled)
}

async fn prepare_input_jobs(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    files: Vec<PathBuf>,
) -> Result<Vec<FileJob>> {
    let total = files.len();
    let mut tasks = JoinSet::new();
    let mut files = files.into_iter();
    let workers = limits.metadata.available_permits().max(1);
    let mut prepared = Vec::new();
    loop {
        while tasks.len() < workers {
            let Some(path) = files.next() else {
                break;
            };
            let state = state.clone();
            let limits = limits.clone();
            tasks.spawn(async move { prepare_input(&state, &limits, path).await });
        }
        let Some(result) = tasks.join_next().await else {
            break;
        };
        prepared.push(result.context("input duplicate-analysis worker failed")?);
        state
            .set_workflow(
                WorkflowPhase::Fetch,
                "deduplicate",
                "Analyzing input recordings for duplicates",
                prepared.len(),
                total,
                prepared
                    .last()
                    .map(|input| input.path.to_string_lossy().into_owned()),
            )
            .await;
        if state.workflow_cancelled().await {
            tasks.abort_all();
            break;
        }
    }

    let evidence = prepared
        .iter()
        .map(|input| RecordingEvidence {
            path: input.path.clone(),
            format: input
                .info
                .as_ref()
                .map(|info| info.format.clone())
                .unwrap_or_default(),
            bitrate: input.info.as_ref().and_then(|info| info.bitrate),
            duration: input.info.as_ref().map(|info| info.duration),
            content_key: input.content_key.clone(),
            isrc: None,
        })
        .collect::<Vec<_>>();
    Ok(input_dedup::group_recordings(&evidence)
        .into_iter()
        .map(|group| FileJob {
            members: group
                .members
                .into_iter()
                .map(|index| prepared[index].clone())
                .collect(),
        })
        .collect())
}

async fn prepare_input(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    path: PathBuf,
) -> PreparedInput {
    let info = {
        let permit = limits.metadata.acquire().await;
        match permit {
            Ok(_permit) => tokio::task::spawn_blocking({
                let path = path.clone();
                move || audio::read(&path)
            })
            .await
            .ok()
            .and_then(Result::ok),
            Err(_) => None,
        }
    };
    let healthy = if info.is_some() {
        !matches!(
            crate::infrastructure::media::integrity::check(&state.pool, &path).await,
            Ok(crate::infrastructure::media::integrity::Integrity::Corrupt(
                _
            ))
        )
    } else {
        false
    };
    let content_key = if healthy {
        let permit = limits.fingerprint.acquire().await;
        match permit {
            Ok(_permit) => match fingerprint_cache::get_or_calculate(&state.pool, &path, || async {
                fingerprint::calculate(&path).await
            })
            .await
            {
                Ok(value) => input_dedup::fingerprint_key(&value.fingerprint),
                Err(_) => input_dedup::sha256_cached(&state.pool, &path)
                    .await
                    .ok()
                    .map(|hash| input_dedup::hash_key(&hash)),
            },
            Err(_) => None,
        }
    } else {
        None
    };
    PreparedInput {
        path,
        info,
        content_key,
    }
}

async fn finish_scan_progress(state: &Arc<AppState>, total: usize) {
    let processed = state.finish_track(total).await;
    state
        .set_workflow(
            WorkflowPhase::Fetch,
            "fetch",
            "Matching tracks",
            processed,
            total,
            None,
        )
        .await;
}

pub(super) struct PipelineLimits {
    pub(super) metadata: Arc<Semaphore>,
    pub(super) fingerprint: Arc<Semaphore>,
    pub(super) acoustid: Arc<Semaphore>,
    pub(super) songrec: Arc<Semaphore>,
    pub(super) disabled_providers: Arc<Mutex<HashSet<String>>>,
}

#[derive(Clone)]
pub(super) struct FileJob {
    pub(super) members: Vec<PreparedInput>,
}

#[derive(Clone)]
pub(super) struct PreparedInput {
    pub(super) path: PathBuf,
    pub(super) info: Option<audio::AudioInfo>,
    pub(super) content_key: Option<String>,
}

pub(super) struct PersistJob {
    pub(super) path: PathBuf,
    pub(super) info: audio::AudioInfo,
    pub(super) candidate: infra_providers::Candidate,
    pub(super) message: String,
    pub(super) result: oneshot::Sender<Result<()>>,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub(super) enum ProcessOutcome {
    Matched,
    NeedsReview,
    Corrupt,
}

#[derive(Clone, Copy)]
pub(super) struct FingerprintEvidence<'a> {
    pub(super) value: &'a str,
    pub(super) duration: f64,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn automatic_scan_only_returns_new_or_changed_files() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("automatic-files.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let source = directory.path().join("song.mp3");
        tokio::fs::write(&source, b"first version").await.unwrap();
        let source = source.canonicalize().unwrap();

        let pending = automatic_pending_files(&pool, vec![source.clone()])
            .await
            .unwrap();
        assert_eq!(pending, vec![source.clone()]);

        remember_scanned_files(&pool, std::slice::from_ref(&source))
            .await
            .unwrap();
        assert!(
            automatic_pending_files(&pool, vec![source.clone()])
                .await
                .unwrap()
                .is_empty()
        );

        tokio::fs::write(&source, b"a changed and longer version")
            .await
            .unwrap();
        let pending = automatic_pending_files(&pool, vec![source.clone()])
            .await
            .unwrap();
        assert_eq!(pending, vec![source]);
    }

    async fn test_pool() -> sqlx::SqlitePool {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("scan-pipeline.sqlite");
        let pool = crate::infrastructure::db::connect(path.to_str().unwrap())
            .await
            .unwrap();
        std::mem::forget(dir);
        pool
    }

    #[tokio::test]
    async fn persist_unmatched_upserts_review_track_with_metadata() {
        let pool = test_pool().await;
        let path = Path::new("/music/input/unmatched.mp3");
        let info = audio::AudioInfo {
            title: Some("Current title".into()),
            artist: Some("Current artist".into()),
            album: Some("Current album".into()),
            album_artist: Some("Current album artist".into()),
            track_number: Some(7),
            genre: Some("Rock".into()),
            duration: 181.4,
            bitrate: Some(320),
            format: "mp3".into(),
        };

        persist_unmatched(&pool, path, &info, "No candidate met threshold 90")
            .await
            .unwrap();

        let row: (String, String, Option<String>, Option<String>, Option<i64>) =
            sqlx::query_as("SELECT stage,status,stage_message,current_title,selected_candidate_id FROM tracks WHERE path=?")
                .bind(path.to_string_lossy().as_ref())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, "review");
        assert_eq!(row.1, "needs_review");
        assert_eq!(row.2.as_deref(), Some("No candidate met threshold 90"));
        assert_eq!(row.3.as_deref(), Some("Current title"));
        assert_eq!(row.4, None);
    }

    #[tokio::test]
    async fn persist_failed_upserts_failed_track_with_error() {
        let pool = test_pool().await;
        let path = Path::new("/music/input/broken.flac");
        persist_failed(&pool, path, "fpcalc failed: command not found")
            .await
            .unwrap();

        let row: (String, String, Option<String>, Option<String>, Option<i64>) =
            sqlx::query_as("SELECT stage,status,stage_message,error,selected_candidate_id FROM tracks WHERE path=?")
                .bind(path.to_string_lossy().as_ref())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, "failed");
        assert_eq!(row.1, "provider_error");
        assert_eq!(row.2.as_deref(), Some("Track failed after retries"));
        assert_eq!(row.3.as_deref(), Some("fpcalc failed: command not found"));
        assert_eq!(row.4, None);
    }

    #[tokio::test]
    async fn persist_corrupt_blocks_selection_and_keeps_readable_metadata() {
        let pool = test_pool().await;
        let path = Path::new("/music/input/damaged.mp3");
        let info = audio::AudioInfo {
            title: Some("Readable title".into()),
            artist: Some("Readable artist".into()),
            duration: 42.0,
            format: "mp3".into(),
            ..Default::default()
        };
        persist_corrupt(&pool, path, &info, "Invalid frame header")
            .await
            .unwrap();

        let row: (String, String, Option<String>, Option<String>, Option<i64>) =
            sqlx::query_as("SELECT stage,status,error,current_title,selected_candidate_id FROM tracks WHERE path=?")
                .bind(path.to_string_lossy().as_ref())
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(row.0, "failed");
        assert_eq!(row.1, "corrupt");
        assert_eq!(row.2.as_deref(), Some("Invalid frame header"));
        assert_eq!(row.3.as_deref(), Some("Readable title"));
        assert_eq!(row.4, None);
    }

    #[tokio::test]
    async fn persist_review_stores_candidate_source_evidence() {
        let pool = test_pool().await;
        let path = Path::new("/music/input/review.mp3");
        let info = audio::AudioInfo {
            title: Some("Song".into()),
            artist: Some("Artist".into()),
            duration: 180.0,
            format: "mp3".into(),
            ..Default::default()
        };
        let candidates = vec![infra_providers::Candidate {
            provider: "musicbrainz".into(),
            title: "Song".into(),
            artist: "Artist".into(),
            album: Some("Album".into()),
            recording_id: Some("rec-1".into()),
            release_id: Some("rel-1".into()),
            score: 91.0,
            raw_json: "{}".into(),
            score_breakdown: Some(serde_json::json!({"acoustid":0.96}).to_string()),
            ..Default::default()
        }];

        persist_review(&pool, path, &info, &candidates, "Review required")
            .await
            .unwrap();

        let providers: Vec<String> =
            sqlx::query_scalar("SELECT provider FROM candidate_sources ORDER BY provider")
                .fetch_all(&pool)
                .await
                .unwrap();
        assert_eq!(providers, ["AcoustID", "MusicBrainz"]);
    }

    #[test]
    fn source_agreement_adds_provider_evidence_and_score() {
        let mut candidates = vec![
            infra_providers::Candidate {
                provider: "discogs".into(),
                title: "Song".into(),
                artist: "Artist".into(),
                album: Some("Album".into()),
                score: 82.0,
                score_breakdown: Some(serde_json::json!({"sources":["Discogs"]}).to_string()),
                ..Default::default()
            },
            infra_providers::Candidate {
                provider: "lastfm".into(),
                title: "Song".into(),
                artist: "Artist".into(),
                album: Some("Album".into()),
                score: 80.0,
                score_breakdown: Some(serde_json::json!({"sources":["Last.fm"]}).to_string()),
                ..Default::default()
            },
        ];

        apply_source_agreement(&mut candidates).unwrap();

        assert!(candidates[0].score > 82.0);
        let why: serde_json::Value =
            serde_json::from_str(candidates[0].score_breakdown.as_deref().unwrap()).unwrap();
        assert!(
            why["sources"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("Discogs"))
        );
        assert!(
            why["sources"]
                .as_array()
                .unwrap()
                .contains(&serde_json::json!("Last.fm"))
        );
    }

    #[test]
    fn artwork_fallbacks_keep_matching_catalog_covers_only() {
        let mut candidates = vec![
            infra_providers::Candidate {
                provider: "musicbrainz".into(),
                title: "Song".into(),
                artist: "Artist".into(),
                album: Some("Album".into()),
                cover_url: Some("https://example.test/musicbrainz.jpg".into()),
                score: 94.0,
                ..Default::default()
            },
            infra_providers::Candidate {
                provider: "itunes".into(),
                title: "Song".into(),
                artist: "Artist".into(),
                album: Some("Album".into()),
                cover_url: Some("https://example.test/correct.jpg".into()),
                score: 91.0,
                ..Default::default()
            },
            infra_providers::Candidate {
                provider: "youtube".into(),
                title: "Song".into(),
                artist: "Different Artist".into(),
                cover_url: Some("https://example.test/wrong.jpg".into()),
                score: 60.0,
                ..Default::default()
            },
        ];

        enrich_artwork_fallbacks(&mut candidates).unwrap();

        assert_eq!(
            candidates[0].cover_url.as_deref(),
            Some("https://example.test/correct.jpg")
        );
        let details: serde_json::Value =
            serde_json::from_str(candidates[0].score_breakdown.as_deref().unwrap()).unwrap();
        let serialized = details["artwork_candidates"].to_string();
        assert!(serialized.contains("correct.jpg"));
        assert!(!serialized.contains("wrong.jpg"));
    }

    #[test]
    fn provider_only_candidate_without_agreement_is_not_safe_auto_select() {
        let candidate = infra_providers::Candidate {
            provider: "discogs".into(),
            title: "Song".into(),
            artist: "Artist".into(),
            score: 88.0,
            duration_delta: Some(1.0),
            score_breakdown: Some(
                serde_json::json!({
                    "acoustid": 0.0,
                    "sources": ["Discogs"]
                })
                .to_string(),
            ),
            ..Default::default()
        };

        assert!(!candidate_has_fingerprint(&candidate));
        assert_eq!(candidate_source_count(&candidate), 1);
        assert!(!crate::domain::matcher::auto_selectable(
            candidate.score,
            None,
            candidate.duration_delta
        ));
        assert!(
            !(candidate_has_fingerprint(&candidate) || candidate_source_count(&candidate) >= 2)
        );
    }

    #[test]
    fn fingerprint_backed_musicbrainz_candidate_outranks_noisy_external_results() {
        let mut candidates = vec![
            infra_providers::Candidate {
                provider: "lastfm".into(),
                title: "Out of Time".into(),
                artist: "The Weeknd".into(),
                score: 96.0,
                duration_delta: Some(1.0),
                score_breakdown: Some(serde_json::json!({"sources":["Last.fm"]}).to_string()),
                ..Default::default()
            },
            infra_providers::Candidate {
                provider: "musicbrainz".into(),
                title: "Out of Time".into(),
                artist: "The Weeknd".into(),
                album: Some("Dawn FM".into()),
                score: 94.0,
                duration_delta: Some(1.0),
                score_breakdown: Some(
                    serde_json::json!({
                        "acoustid": 0.98,
                        "sources": ["AcoustID", "MusicBrainz"]
                    })
                    .to_string(),
                ),
                ..Default::default()
            },
        ];

        sort_candidates_for_decision(&mut candidates);

        assert_eq!(candidates[0].provider, "musicbrainz");
        assert!(candidate_has_fingerprint(&candidates[0]));
        let decision = crate::application::smart_approval::rank(
            crate::application::smart_approval::TrackEvidence {
                filename: "The Weeknd - Out of Time.mp3",
                title: Some("Out of Time"),
                artist: Some("The Weeknd"),
                album: Some("Dawn FM"),
            },
            &candidates,
        )
        .unwrap();
        assert_eq!(candidates[decision.candidate_index].provider, "musicbrainz");
    }

    #[test]
    fn dedupe_candidates_keeps_strongest_duplicate() {
        let mut candidates = vec![
            infra_providers::Candidate {
                provider: "lastfm".into(),
                title: "Song!".into(),
                artist: "Artist".into(),
                album: Some("Album".into()),
                score: 72.0,
                ..Default::default()
            },
            infra_providers::Candidate {
                provider: "discogs".into(),
                title: "Song".into(),
                artist: "Artist".into(),
                album: Some("Album".into()),
                score: 80.0,
                ..Default::default()
            },
        ];

        dedupe_candidates(&mut candidates);

        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates[0].provider, "discogs");
    }

    #[tokio::test]
    async fn persist_review_keeps_non_musicbrainz_provider() {
        let pool = test_pool().await;
        let path = Path::new("/music/input/discogs-review.mp3");
        let info = audio::AudioInfo {
            title: Some("Song".into()),
            artist: Some("Artist".into()),
            duration: 180.0,
            format: "mp3".into(),
            ..Default::default()
        };
        let candidates = vec![infra_providers::Candidate {
            provider: "discogs".into(),
            title: "Song".into(),
            artist: "Artist".into(),
            score: 84.0,
            raw_json: "{}".into(),
            ..Default::default()
        }];

        persist_review(&pool, path, &info, &candidates, "Review required")
            .await
            .unwrap();

        let provider: String = sqlx::query_scalar("SELECT provider FROM candidates")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(provider, "discogs");
    }

    #[test]
    fn equivalent_catalog_releases_are_one_exact_match() {
        let info = audio::AudioInfo {
            title: Some("Колыбельная".into()),
            artist: Some("Jah Khalib".into()),
            album: Some("@radiop0l".into()),
            ..Default::default()
        };
        let candidates = vec![
            infra_providers::Candidate {
                provider: "itunes".into(),
                title: "Колыбельная".into(),
                artist: "Jah Khalib".into(),
                album: Some("E.G.O.".into()),
                duration_delta: Some(0.14),
                ..Default::default()
            },
            infra_providers::Candidate {
                provider: "musicbrainz".into(),
                title: "Колыбельная".into(),
                artist: "Jah Khalib".into(),
                album: Some("E.G.O.".into()),
                duration_delta: Some(0.04),
                ..Default::default()
            },
        ];
        assert!(unique_exact_catalog_match(
            &candidates[0],
            &candidates,
            &info
        ));
    }

    #[test]
    fn conflicting_existing_album_blocks_unique_text_match() {
        let info = audio::AudioInfo {
            title: Some("All of the Stars".into()),
            artist: Some("Ed Sheeran".into()),
            album: Some("The Fault In Our Stars (Music From The Motion Picture)".into()),
            ..Default::default()
        };
        let candidates = vec![infra_providers::Candidate {
            provider: "itunes".into(),
            title: "All of the Stars".into(),
            artist: "Ed Sheeran".into(),
            album: Some("x (10th Anniversary Edition)".into()),
            duration_delta: Some(2.0),
            ..Default::default()
        }];
        assert!(!unique_exact_catalog_match(
            &candidates[0],
            &candidates,
            &info
        ));
    }

    #[test]
    fn exact_single_keeps_known_album_track_context() {
        let info = audio::AudioInfo {
            title: Some("Khaar".into()),
            artist: Some("Amir Tataloo".into()),
            album: Some("Barzakh".into()),
            track_number: Some(11),
            duration: 427.102,
            ..Default::default()
        };
        let mut candidate = infra_providers::Candidate {
            provider: "itunes".into(),
            title: "Khaar".into(),
            artist: "Amir Tataloo".into(),
            album: Some("Khaar - Single".into()),
            track_number: Some(1),
            track_total: Some(1),
            duration_delta: Some(427.076),
            release_id: Some("single-release".into()),
            ..Default::default()
        };
        preserve_album_context_for_catalog_single(&mut candidate, &info);
        assert_eq!(candidate.album.as_deref(), Some("Barzakh"));
        assert_eq!(candidate.track_number, Some(11));
        assert_eq!(candidate.track_total, None);
        assert_eq!(candidate.release_id, None);
    }

    #[test]
    fn ignores_filename_filler_words_when_matching_title() {
        assert!(title_similarity("Such A Lonely Day", "Lonely Day") >= 0.94);
    }

    #[test]
    fn reported_lonely_day_catalog_match_scores_as_reliable() {
        let info = audio::AudioInfo {
            title: Some("Such A Lonely Day".into()),
            artist: Some("System Of A Down".into()),
            duration: 167.993,
            ..Default::default()
        };
        let mut candidate = infra_providers::Candidate {
            provider: "itunes".into(),
            title: "Lonely Day".into(),
            artist: "System Of A Down".into(),
            album: Some("Hypnotize".into()),
            duration_delta: Some(167.907),
            ..Default::default()
        };

        score_text_candidate(&mut candidate, &info, "itunes_catalog_search").unwrap();

        assert!(candidate.score >= 90.0);
        assert!(candidate.duration_delta.is_some_and(|delta| delta < 1.0));
    }

    #[test]
    fn genius_is_reserved_for_incomplete_or_weak_catalog_results() {
        let complete = infra_providers::Candidate {
            score: 94.0,
            album: Some("Hypnotize".into()),
            cover_url: Some("https://example.test/hypnotize.jpg".into()),
            ..Default::default()
        };
        let missing_cover = infra_providers::Candidate {
            cover_url: None,
            ..complete.clone()
        };
        assert!(!needs_genius_enrichment(&[complete]));
        assert!(needs_genius_enrichment(&[missing_cover]));
        assert!(needs_genius_enrichment(&[]));
    }
}
