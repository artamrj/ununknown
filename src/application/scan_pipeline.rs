use crate::{
    app::{ActivityLogEntry, AppState},
    domain::audio,
    infrastructure::{fingerprint_cache, media::fingerprint, providers},
    types::{AutomationMode, WorkflowPhase},
};
use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::{Duration, Instant},
};
use tokio::{
    sync::{Mutex, Semaphore, mpsc, oneshot},
    task::JoinSet,
};
use walkdir::WalkDir;

struct PipelineLimits {
    metadata: Arc<Semaphore>,
    fingerprint: Arc<Semaphore>,
    acoustid: Arc<Semaphore>,
}

#[derive(Clone)]
struct FileJob {
    index: usize,
    path: PathBuf,
}

struct PersistJob {
    path: PathBuf,
    info: audio::AudioInfo,
    candidate: providers::Candidate,
    result: oneshot::Sender<Result<()>>,
}

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
    let mut walk_errors = Vec::new();
    let mut files: Vec<PathBuf> = WalkDir::new(&cfg.input_dir)
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
    state
        .set_workflow(
            WorkflowPhase::Fetch,
            "fetch",
            "Starting staged matching",
            0,
            total,
            None,
        )
        .await;

    let limits = Arc::new(PipelineLimits {
        metadata: Arc::new(Semaphore::new(cfg.metadata_read_concurrency)),
        fingerprint: Arc::new(Semaphore::new(cfg.fingerprint_concurrency)),
        acoustid: Arc::new(Semaphore::new(cfg.acoustid_concurrency)),
    });
    let (persist_tx, persist_rx) = mpsc::channel(cfg.db_write_batch_size.max(1));
    let writer = tokio::spawn(db_writer(
        state.clone(),
        persist_rx,
        cfg.db_write_batch_size,
    ));
    let scan_workers = cfg.scan_worker_concurrency.max(1).min(total.max(1));
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
    for (index, path) in files.into_iter().enumerate() {
        if state.workflow_cancelled().await {
            break;
        }
        file_tx.send(FileJob { index, path }).await?;
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
                "Matching complete"
            },
        )
        .await;
    Ok(())
}

async fn process_file(
    state: Arc<AppState>,
    limits: Arc<PipelineLimits>,
    persist_tx: mpsc::Sender<PersistJob>,
    job: FileJob,
    total: usize,
) {
    if state.workflow_cancelled().await {
        return;
    }
    let cfg = state.config.read().await.clone();
    let filename = job
        .path
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("audio")
        .to_owned();
    for attempt in 1..=cfg.track_attempts {
        if state.workflow_cancelled().await {
            return;
        }
        state
            .set_workflow(
                WorkflowPhase::Fetch,
                "fetch",
                format!(
                    "Matching {filename} · attempt {attempt}/{}",
                    cfg.track_attempts
                ),
                job.index,
                total,
                Some(filename.clone()),
            )
            .await;
        match process(&state, &limits, &persist_tx, &job.path).await {
            Ok(true) => {
                state
                    .log(
                        "ok",
                        "fetch",
                        Some(&filename),
                        "Matched and stored for Preview",
                    )
                    .await;
                break;
            }
            Ok(false) => {
                state
                    .log(
                        "warn",
                        "fetch",
                        Some(&filename),
                        "No selected match; moving to next file",
                    )
                    .await;
                break;
            }
            Err(error) if attempt < cfg.track_attempts => {
                tracing::warn!(path=%job.path.display(), attempt, "track attempt failed: {error:#}");
                state
                    .log_entry(
                        ActivityLogEntry::new("warn", "fetch", "Track attempt failed; retrying")
                            .file(filename.clone())
                            .attempt(attempt as i64)
                            .error_text(format!("{error:#}"))
                            .context(serde_json::json!({
                                "path": job.path.display().to_string(),
                                "max_attempts": cfg.track_attempts
                            })),
                    )
                    .await;
                tokio::time::sleep(Duration::from_millis(400 * attempt as u64)).await;
            }
            Err(error) => {
                tracing::warn!(path=%job.path.display(), "track failed after retries: {error:#}");
                state.increment_failed().await;
                let error_text = format!("{error:#}");
                if let Err(persist_error) =
                    persist_failed(&state.pool, &job.path, &error_text).await
                {
                    tracing::warn!(path=%job.path.display(), "failed to persist failed track: {persist_error:#}");
                    state
                        .log_entry(
                            ActivityLogEntry::new("error", "db", "Failed to persist failed track")
                                .file(filename.clone())
                                .error_text(format!("{persist_error:#}")),
                        )
                        .await;
                }
                state
                    .log_entry(
                        ActivityLogEntry::new("error", "fetch", "Track failed after retries")
                            .file(filename.clone())
                            .attempt(attempt as i64)
                            .error_text(error_text)
                            .context(serde_json::json!({
                                "path": job.path.display().to_string(),
                                "max_attempts": cfg.track_attempts
                            })),
                    )
                    .await;
            }
        }
    }
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

async fn process(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    persist_tx: &mpsc::Sender<PersistJob>,
    path: &Path,
) -> Result<bool> {
    let filename = path.file_name().and_then(|v| v.to_str()).unwrap_or("audio");
    state
        .log(
            "info",
            "metadata",
            Some(filename),
            "Reading existing tags and audio properties",
        )
        .await;
    let started = Instant::now();
    let info = {
        let _permit = limits.metadata.acquire().await?;
        tokio::task::spawn_blocking({
            let p = path.to_path_buf();
            move || audio::read(&p)
        })
        .await
        .context("metadata reader task failed")?
        .with_context(|| format!("failed to read metadata from {}", path.display()))?
    };
    state
        .log(
            "ok",
            "metadata",
            Some(filename),
            &format!(
                "Read {} · {}s · current title: {}",
                info.format,
                info.duration.round(),
                info.title.as_deref().unwrap_or("missing")
            ),
        )
        .await;
    state
        .log_entry(
            ActivityLogEntry::new("info", "metadata", "Metadata read timing")
                .file(filename.to_owned())
                .duration_ms(started.elapsed().as_millis() as i64),
        )
        .await;
    state
        .log(
            "info",
            "fingerprint",
            Some(filename),
            "Running fpcalc fingerprint",
        )
        .await;
    let started = Instant::now();
    let fingerprint_result = {
        let _permit = limits.fingerprint.acquire().await?;
        fingerprint_cache::get_or_calculate(&state.pool, path, || async {
            fingerprint::calculate(path)
                .await
                .with_context(|| format!("failed to fingerprint {}", path.display()))
        })
        .await?
    };
    let (fp, duration) = (fingerprint_result.fingerprint, fingerprint_result.duration);
    let (message, detail) = match fingerprint_result.source {
        fingerprint_cache::FingerprintSource::Cache => (
            "Fingerprint reused from cache",
            "File size and modified time matched the cached fingerprint",
        ),
        fingerprint_cache::FingerprintSource::Generated => (
            "Fingerprint generated",
            "Cached fingerprint was missing or stale",
        ),
    };
    state
        .log_entry(
            ActivityLogEntry::new("ok", "fingerprint", message)
                .file(filename.to_owned())
                .detail(detail)
                .duration_ms(started.elapsed().as_millis() as i64),
        )
        .await;
    let cfg = state.config.read().await.clone();
    if cfg.acoustid_api_key.is_empty() {
        state
            .log(
                "warn",
                "acoustid",
                Some(filename),
                "AcoustID key is not configured; using MusicBrainz tag search fallback",
            )
            .await;
    } else {
        state
            .log(
                "info",
                "acoustid",
                Some(filename),
                "Querying AcoustID fingerprint lookup",
            )
            .await;
    }
    state
        .log(
            "info",
            "musicbrainz",
            Some(filename),
            "MusicBrainz lookups are queued at one request per second",
        )
        .await;
    if info.duration < 30.0 {
        state.increment_unmatched().await;
        let message = "Track is shorter than 30 seconds; skipped automatic identification";
        persist_unmatched(&state.pool, path, &info, message).await?;
        state.log("warn", "match", Some(filename), message).await;
        return Ok(false);
    }
    let mut candidates = identify(state, &cfg, limits, &fp, duration, &info, filename).await?;
    state
        .log(
            "info",
            "musicbrainz",
            Some(filename),
            &format!("Provider returned {} candidate(s)", candidates.len()),
        )
        .await;
    candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
    if cfg.automation_mode == AutomationMode::Manual {
        state.increment_unmatched().await;
        let message = "Manual mode is enabled; candidates require review";
        if candidates.is_empty() {
            persist_unmatched(&state.pool, path, &info, message).await?;
        } else {
            persist_review(&state.pool, path, &info, &candidates, message).await?;
        }
        state.log("warn", "match", Some(filename), message).await;
        return Ok(false);
    }
    let Some(best) = candidates.first() else {
        state.increment_unmatched().await;
        let message = "No MusicBrainz candidate found; counted as unmatched";
        persist_unmatched(&state.pool, path, &info, &message).await?;
        state.log("warn", "match", Some(filename), &message).await;
        return Ok(false);
    };
    let second_score = candidates.get(1).map(|candidate| candidate.score);
    let auto_select =
        crate::domain::matcher::auto_selectable(best.score, second_score, best.duration_delta);
    if !auto_select {
        state.increment_unmatched().await;
        let message = if best.score >= 80.0 {
            "Release uncertain; review required before applying metadata".to_owned()
        } else {
            "No candidate met strict matching rules; counted as unmatched".to_owned()
        };
        if best.score >= 80.0 {
            persist_review(&state.pool, path, &info, &candidates, &message).await?;
        } else {
            persist_unmatched(&state.pool, path, &info, &message).await?;
        }
        state.log("warn", "match", Some(filename), &message).await;
        return Ok(false);
    }
    let candidate = candidates.remove(0);
    state
        .log(
            "ok",
            "match",
            Some(filename),
            &format!(
                "Selected {:.0}% · {} - {}",
                candidate.score, candidate.artist, candidate.title
            ),
        )
        .await;
    let (result_tx, result_rx) = oneshot::channel();
    persist_tx
        .send(PersistJob {
            path: path.to_path_buf(),
            info,
            candidate,
            result: result_tx,
        })
        .await
        .map_err(|_| anyhow!("DB writer stopped"))?;
    result_rx
        .await
        .map_err(|_| anyhow!("DB writer stopped"))??;
    state.increment_matched().await;
    Ok(true)
}

async fn identify(
    state: &Arc<AppState>,
    cfg: &crate::config::Config,
    limits: &Arc<PipelineLimits>,
    fingerprint: &str,
    duration: f64,
    current: &audio::AudioInfo,
    filename: &str,
) -> Result<Vec<providers::Candidate>> {
    let mut out = Vec::new();
    if !cfg.acoustid_api_key.is_empty() {
        let started = Instant::now();
        let hits = {
            let _permit = limits.acoustid.acquire().await?;
            providers::acoustid::lookup(
                &state.pool,
                &state.client,
                &cfg.acoustid_api_key,
                fingerprint,
                duration,
            )
            .await?
        };
        state
            .log_entry(
                ActivityLogEntry::new(
                    "info",
                    "acoustid",
                    format!("AcoustID returned {} hit(s)", hits.len()),
                )
                .file(filename.to_owned())
                .duration_ms(started.elapsed().as_millis() as i64),
            )
            .await;
        for hit in hits.into_iter().take(3) {
            let started = Instant::now();
            let mut candidate = providers::musicbrainz::recording(
                &state.pool,
                &state.client,
                &cfg.musicbrainz_user_agent,
                &hit.recording_id,
            )
            .await?;
            state
                .log_entry(
                    ActivityLogEntry::new("info", "musicbrainz", "Fetched recording details")
                        .file(filename.to_owned())
                        .duration_ms(started.elapsed().as_millis() as i64)
                        .context(serde_json::json!({"recording_id": hit.recording_id})),
                )
                .await;
            let candidate_duration = candidate.duration_delta;
            let duration_delta = candidate_duration.map(|value| (current.duration - value).abs());
            let breakdown = crate::domain::matcher::score(crate::domain::matcher::CandidateInput {
                acoustid_score: hit.score,
                current,
                title: &candidate.title,
                artist: &candidate.artist,
                album: candidate.album.as_deref(),
                candidate_duration,
                is_compilation: candidate.is_compilation,
                compilation_preference: cfg.compilation_preference,
            });
            candidate.score = breakdown.final_score;
            candidate.duration_delta = duration_delta;
            candidate.score_breakdown = Some(serde_json::to_string(&breakdown)?);
            out.push(candidate);
        }
    }
    if out.is_empty() {
        let title = current
            .title
            .as_deref()
            .filter(|value| !value.trim().is_empty());
        if let Some(title) = title {
            let started = Instant::now();
            for mut candidate in providers::musicbrainz::search(
                &state.pool,
                &state.client,
                &cfg.musicbrainz_user_agent,
                title,
                current.artist.as_deref(),
            )
            .await?
            {
                candidate.score = crate::domain::matcher::text_score(
                    current,
                    &candidate.title,
                    &candidate.artist,
                );
                candidate.score_breakdown = Some(
                    serde_json::json!({
                        "source": "musicbrainz_text_search",
                        "final_score": candidate.score
                    })
                    .to_string(),
                );
                out.push(candidate);
            }
            state
                .log_entry(
                    ActivityLogEntry::new(
                        "info",
                        "musicbrainz",
                        format!("Tag search returned {} candidate(s)", out.len()),
                    )
                    .file(filename.to_owned())
                    .duration_ms(started.elapsed().as_millis() as i64)
                    .context(serde_json::json!({"title": title, "artist": current.artist})),
                )
                .await;
        }
    }
    Ok(out)
}

async fn db_writer(
    state: Arc<AppState>,
    mut rx: mpsc::Receiver<PersistJob>,
    batch_size: usize,
) -> Result<()> {
    let batch_size = batch_size.max(1);
    while let Some(first) = rx.recv().await {
        let mut batch = vec![first];
        while batch.len() < batch_size {
            match rx.try_recv() {
                Ok(job) => batch.push(job),
                Err(_) => break,
            }
        }
        let mut tx = state.pool.begin().await?;
        let mut results = Vec::with_capacity(batch.len());
        for job in &batch {
            results.push(persist_match(&mut tx, &job.path, &job.info, &job.candidate).await);
        }
        let failed = results
            .iter()
            .find_map(|result| result.as_ref().err().map(|error| anyhow!("{error:#}")));
        let failed = match failed {
            Some(error) => Some(error),
            None => tx.commit().await.err().map(|error| anyhow!("{error:#}")),
        };
        if let Some(error) = &failed {
            state
                .log_entry(
                    ActivityLogEntry::new("error", "db", "Failed to persist matched candidates")
                        .error(error.as_ref())
                        .context(serde_json::json!({"batch_size": batch.len()})),
                )
                .await;
        }
        for (job, result) in batch.into_iter().zip(results) {
            let response = match (&failed, result) {
                (Some(error), _) => Err(anyhow!("{error:#}")),
                (None, Ok(())) => Ok(()),
                (None, Err(error)) => Err(error),
            };
            let _ = job.result.send(response);
        }
    }
    Ok(())
}

async fn persist_match(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    path: &Path,
    info: &audio::AudioInfo,
    c: &providers::Candidate,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let text = path.to_string_lossy();
    let filename = path
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or_else(|| anyhow!("invalid filename"))?;
    let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM tracks WHERE path=?")
        .bind(text.as_ref())
        .fetch_optional(&mut **tx)
        .await?;
    let id = if let Some(id) = existing {
        sqlx::query("DELETE FROM candidates WHERE track_id=?")
            .bind(id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("UPDATE tracks SET filename=?,format=?,duration=?,current_title=?,current_artist=?,current_album=?,current_album_artist=?,current_track_number=?,status='selected',error=NULL,is_missing=0,last_seen_at=?,last_scanned_at=?,stage='ready',updated_at=?,selected_candidate_id=NULL WHERE id=?")
            .bind(filename).bind(&info.format).bind(info.duration).bind(&info.title).bind(&info.artist).bind(&info.album).bind(&info.album_artist).bind(info.track_number.map(i64::from)).bind(&now).bind(&now).bind(&now).bind(id).execute(&mut **tx).await?;
        id
    } else {
        sqlx::query("INSERT INTO tracks(path,filename,format,duration,current_title,current_artist,current_album,current_album_artist,current_track_number,status,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage,updated_at) VALUES(?,?,?,?,?,?,?,?,?,'selected',0,?,?,?,'ready',?)")
            .bind(text.as_ref()).bind(filename).bind(&info.format).bind(info.duration).bind(&info.title).bind(&info.artist).bind(&info.album).bind(&info.album_artist).bind(info.track_number.map(i64::from)).bind(&now).bind(&now).bind(&now).bind(&now).execute(&mut **tx).await?.last_insert_rowid()
    };
    let cid = insert_candidate(tx, id, c).await?;
    sqlx::query("UPDATE tracks SET selected_candidate_id=? WHERE id=?")
        .bind(cid)
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn insert_candidate(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    track_id: i64,
    c: &providers::Candidate,
) -> Result<i64> {
    Ok(sqlx::query("INSERT INTO candidates(track_id,provider,title,artist,album,album_artist,track_number,track_total,disc_number,disc_total,year,genre,composer,label,isrc,cover_url,musicbrainz_recording_id,musicbrainz_release_id,release_country,release_date,release_type,release_secondary_types,is_compilation,duration_delta,score_breakdown,musicbrainz_artist_id,musicbrainz_album_artist_id,score,raw_json) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(track_id)
        .bind("musicbrainz")
        .bind(&c.title)
        .bind(&c.artist)
        .bind(&c.album)
        .bind(&c.album_artist)
        .bind(c.track_number)
        .bind(c.track_total)
        .bind(c.disc_number)
        .bind(c.disc_total)
        .bind(&c.year)
        .bind(&c.genre)
        .bind(&c.composer)
        .bind(&c.label)
        .bind(&c.isrc)
        .bind(&c.cover_url)
        .bind(&c.recording_id)
        .bind(&c.release_id)
        .bind(&c.release_country)
        .bind(&c.release_date)
        .bind(&c.release_type)
        .bind(&c.release_secondary_types)
        .bind(c.is_compilation)
        .bind(c.duration_delta)
        .bind(&c.score_breakdown)
        .bind(&c.artist_id)
        .bind(&c.album_artist_id)
        .bind(c.score)
        .bind(&c.raw_json)
        .execute(&mut **tx)
        .await?
        .last_insert_rowid())
}

async fn persist_review(
    pool: &sqlx::SqlitePool,
    path: &Path,
    info: &audio::AudioInfo,
    candidates: &[providers::Candidate],
    message: &str,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let id = upsert_track_outcome(
        &mut tx,
        path,
        Some(info),
        "needs_review",
        "review",
        Some(message),
        None,
    )
    .await?;
    sqlx::query("DELETE FROM candidates WHERE track_id=?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for candidate in candidates.iter().take(5) {
        insert_candidate(&mut tx, id, candidate).await?;
    }
    tx.commit().await?;
    Ok(())
}

async fn persist_unmatched(
    pool: &sqlx::SqlitePool,
    path: &Path,
    info: &audio::AudioInfo,
    message: &str,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let id = upsert_track_outcome(
        &mut tx,
        path,
        Some(info),
        "needs_review",
        "review",
        Some(message),
        None,
    )
    .await?;
    sqlx::query("DELETE FROM candidates WHERE track_id=?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

async fn persist_failed(pool: &sqlx::SqlitePool, path: &Path, error: &str) -> Result<()> {
    let mut tx = pool.begin().await?;
    let id = upsert_track_outcome(
        &mut tx,
        path,
        None,
        "provider_error",
        "failed",
        Some("Track failed after retries"),
        Some(error),
    )
    .await?;
    sqlx::query("DELETE FROM candidates WHERE track_id=?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

async fn upsert_track_outcome(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    path: &Path,
    info: Option<&audio::AudioInfo>,
    status: &str,
    stage: &str,
    stage_message: Option<&str>,
    error: Option<&str>,
) -> Result<i64> {
    let now = Utc::now().to_rfc3339();
    let text = path.to_string_lossy();
    let filename = path
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or_else(|| anyhow!("invalid filename"))?;
    let format = info
        .map(|info| info.format.clone())
        .or_else(|| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase)
        })
        .unwrap_or_default();
    let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM tracks WHERE path=?")
        .bind(text.as_ref())
        .fetch_optional(&mut **tx)
        .await?;
    if let Some(id) = existing {
        sqlx::query("UPDATE tracks SET filename=?,format=?,duration=?,current_title=?,current_artist=?,current_album=?,current_album_artist=?,current_track_number=?,selected_candidate_id=NULL,status=?,error=?,is_missing=0,last_seen_at=?,last_scanned_at=?,stage=?,stage_message=?,updated_at=? WHERE id=?")
            .bind(filename)
            .bind(&format)
            .bind(info.map(|info| info.duration))
            .bind(info.and_then(|info| info.title.as_ref()))
            .bind(info.and_then(|info| info.artist.as_ref()))
            .bind(info.and_then(|info| info.album.as_ref()))
            .bind(info.and_then(|info| info.album_artist.as_ref()))
            .bind(info.and_then(|info| info.track_number.map(i64::from)))
            .bind(status)
            .bind(error)
            .bind(&now)
            .bind(&now)
            .bind(stage)
            .bind(stage_message)
            .bind(&now)
            .bind(id)
            .execute(&mut **tx)
            .await?;
        Ok(id)
    } else {
        let id = sqlx::query("INSERT INTO tracks(path,filename,format,duration,current_title,current_artist,current_album,current_album_artist,current_track_number,selected_candidate_id,status,error,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage,stage_message,updated_at) VALUES(?,?,?,?,?,?,?,?,?,NULL,?,?,0,?,?,?,?,?,?)")
            .bind(text.as_ref())
            .bind(filename)
            .bind(&format)
            .bind(info.map(|info| info.duration))
            .bind(info.and_then(|info| info.title.as_ref()))
            .bind(info.and_then(|info| info.artist.as_ref()))
            .bind(info.and_then(|info| info.album.as_ref()))
            .bind(info.and_then(|info| info.album_artist.as_ref()))
            .bind(info.and_then(|info| info.track_number.map(i64::from)))
            .bind(status)
            .bind(error)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .bind(stage)
            .bind(stage_message)
            .bind(&now)
            .execute(&mut **tx)
            .await?
            .last_insert_rowid();
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

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
}
