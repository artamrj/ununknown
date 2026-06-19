use crate::{
    app::{AppState, TerminalEntry},
    domain::audio,
    infrastructure::{media::fingerprint, providers},
    jobs,
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
    set_phase(&state, WorkflowPhase::Scan, "Discovering music", 0, 0, None).await;
    state
        .terminal(
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
        .terminal(
            "info",
            "scan",
            None,
            &format!("Discovered {total} supported audio files"),
        )
        .await;
    for error in walk_errors {
        state
            .terminal_entry(
                TerminalEntry::new("error", "scan", "Input folder walk error").error_text(error),
            )
            .await;
    }
    if total == 0 {
        state
            .terminal_entry(
                TerminalEntry::new("warn", "scan", "No supported audio files found")
                    .detail(format!("Input folder scanned: {}", cfg.input_dir)),
            )
            .await;
    }
    {
        let mut w = state.workflow.write().await;
        w.total = total;
        w.phase = WorkflowPhase::Fetch;
        w.message = "Starting staged matching".into();
    }
    jobs::emit(
        &state,
        "workflow",
        Some("fetch"),
        Some(WorkflowPhase::Fetch),
        0,
        total as i64,
        "Starting staged matching",
    );

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
        if state.workflow.read().await.cancelled {
            break;
        }
        file_tx.send(FileJob { index, path }).await?;
    }
    drop(file_tx);
    drop(persist_tx);

    while let Some(result) = tasks.join_next().await {
        if let Err(error) = result {
            tracing::warn!("scan worker task failed: {error:#}");
            state.workflow.write().await.failed += 1;
        }
        if state.workflow.read().await.cancelled {
            tasks.abort_all();
        }
    }
    let writer_result = writer.await?;
    writer_result?;
    let cancelled = state.workflow.read().await.cancelled;
    set_phase(
        &state,
        if cancelled {
            WorkflowPhase::Idle
        } else {
            WorkflowPhase::Preview
        },
        if cancelled {
            "Scan stopped"
        } else {
            "Matching complete"
        },
        total,
        total,
        None,
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
    if state.workflow.read().await.cancelled {
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
        if state.workflow.read().await.cancelled {
            return;
        }
        set_phase(
            &state,
            WorkflowPhase::Fetch,
            &format!(
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
                    .terminal(
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
                    .terminal(
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
                    .terminal_entry(
                        TerminalEntry::new("warn", "fetch", "Track attempt failed; retrying")
                            .file(filename.clone())
                            .attempt(attempt as i64)
                            .error(error.as_ref())
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
                state.workflow.write().await.failed += 1;
                state
                    .terminal_entry(
                        TerminalEntry::new("error", "fetch", "Track failed after retries")
                            .file(filename.clone())
                            .attempt(attempt as i64)
                            .error(error.as_ref())
                            .context(serde_json::json!({
                                "path": job.path.display().to_string(),
                                "max_attempts": cfg.track_attempts
                            })),
                    )
                    .await;
            }
        }
    }
    let mut workflow = state.workflow.write().await;
    workflow.processed += 1;
    workflow.current = workflow.processed;
    let processed = workflow.processed;
    drop(workflow);
    jobs::emit(
        &state,
        "workflow",
        Some("fetch"),
        Some(WorkflowPhase::Fetch),
        processed as i64,
        total as i64,
        "Matching tracks",
    );
}

async fn process(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    persist_tx: &mpsc::Sender<PersistJob>,
    path: &Path,
) -> Result<bool> {
    let filename = path.file_name().and_then(|v| v.to_str()).unwrap_or("audio");
    state
        .terminal(
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
        .terminal(
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
        .terminal_entry(
            TerminalEntry::new("info", "metadata", "Metadata read timing")
                .file(filename.to_owned())
                .duration_ms(started.elapsed().as_millis() as i64),
        )
        .await;
    state
        .terminal(
            "info",
            "fingerprint",
            Some(filename),
            "Running fpcalc fingerprint",
        )
        .await;
    let started = Instant::now();
    let (fp, duration) = {
        let _permit = limits.fingerprint.acquire().await?;
        fingerprint::calculate(path)
            .await
            .with_context(|| format!("failed to fingerprint {}", path.display()))?
    };
    state
        .terminal_entry(
            TerminalEntry::new("ok", "fingerprint", "Fingerprint generated")
                .file(filename.to_owned())
                .duration_ms(started.elapsed().as_millis() as i64),
        )
        .await;
    let cfg = state.config.read().await.clone();
    if cfg.acoustid_api_key.is_empty() {
        state
            .terminal(
                "warn",
                "acoustid",
                Some(filename),
                "AcoustID key is not configured; using MusicBrainz tag search fallback",
            )
            .await;
    } else {
        state
            .terminal(
                "info",
                "acoustid",
                Some(filename),
                "Querying AcoustID fingerprint lookup",
            )
            .await;
    }
    state
        .terminal(
            "info",
            "musicbrainz",
            Some(filename),
            "MusicBrainz lookups are queued at one request per second",
        )
        .await;
    let candidates = identify(state, &cfg, limits, &fp, duration, &info, filename).await?;
    state
        .terminal(
            "info",
            "musicbrainz",
            Some(filename),
            &format!("Provider returned {} candidate(s)", candidates.len()),
        )
        .await;
    let threshold = match cfg.automation_mode {
        AutomationMode::Aggressive => 75.0,
        AutomationMode::Manual => 101.0,
        AutomationMode::Custom => cfg.confidence_threshold,
        AutomationMode::Safe => 90.0,
    };
    let best = candidates
        .into_iter()
        .max_by(|a, b| a.score.total_cmp(&b.score));
    let Some(candidate) = best.filter(|c| c.score >= threshold) else {
        state.workflow.write().await.unmatched += 1;
        state
            .terminal(
                "warn",
                "match",
                Some(filename),
                &format!("No candidate met threshold {threshold:.0}; counted as unmatched"),
            )
            .await;
        return Ok(false);
    };
    state
        .terminal(
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
    state.workflow.write().await.matched += 1;
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
            .terminal_entry(
                TerminalEntry::new(
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
                .terminal_entry(
                    TerminalEntry::new("info", "musicbrainz", "Fetched recording details")
                        .file(filename.to_owned())
                        .duration_ms(started.elapsed().as_millis() as i64)
                        .context(serde_json::json!({"recording_id": hit.recording_id})),
                )
                .await;
            candidate.score = crate::domain::matcher::score(
                hit.score,
                current,
                &candidate.title,
                &candidate.artist,
                duration,
            );
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
                out.push(candidate);
            }
            state
                .terminal_entry(
                    TerminalEntry::new(
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
                .terminal_entry(
                    TerminalEntry::new("error", "db", "Failed to persist matched candidates")
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
    let cid=sqlx::query("INSERT INTO candidates(track_id,provider,title,artist,album,album_artist,track_number,track_total,disc_number,disc_total,year,genre,composer,label,isrc,cover_url,musicbrainz_recording_id,musicbrainz_release_id,musicbrainz_artist_id,musicbrainz_album_artist_id,score,raw_json) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(id).bind("musicbrainz").bind(&c.title).bind(&c.artist).bind(&c.album).bind(&c.album_artist).bind(c.track_number).bind(c.track_total).bind(c.disc_number).bind(c.disc_total).bind(&c.year).bind(&c.genre).bind(&c.composer).bind(&c.label).bind(&c.isrc).bind(&c.cover_url).bind(&c.recording_id).bind(&c.release_id).bind(&c.artist_id).bind(&c.album_artist_id).bind(c.score).bind(&c.raw_json).execute(&mut **tx).await?.last_insert_rowid();
    sqlx::query("UPDATE tracks SET selected_candidate_id=? WHERE id=?")
        .bind(cid)
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

async fn set_phase(
    state: &Arc<AppState>,
    phase: WorkflowPhase,
    message: &str,
    current: usize,
    total: usize,
    file: Option<String>,
) {
    let mut w = state.workflow.write().await;
    w.phase = phase;
    w.message = message.into();
    w.current = current;
    w.total = total;
    w.current_file = file;
    drop(w);
    jobs::emit(
        state,
        "workflow",
        Some(phase_name(phase)),
        Some(phase),
        current as i64,
        total as i64,
        message,
    );
}

fn phase_name(phase: WorkflowPhase) -> &'static str {
    match phase {
        WorkflowPhase::Idle => "idle",
        WorkflowPhase::Scan => "scan",
        WorkflowPhase::Fetch => "fetch",
        WorkflowPhase::Preview => "preview",
        WorkflowPhase::Apply => "apply",
        WorkflowPhase::Finish => "finish",
        WorkflowPhase::Failed => "failed",
    }
}
