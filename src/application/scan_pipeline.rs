use crate::{
    app::{ActivityLogEntry, AppState},
    domain::audio,
    infrastructure::{fingerprint_cache, media::fingerprint, providers},
    types::{AutomationMode, MatchingStrategy, ProviderMode, WorkflowPhase},
};
use anyhow::{Context, Result, anyhow};
use chrono::Utc;
use std::{
    collections::HashSet,
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
    disabled_providers: Arc<Mutex<HashSet<String>>>,
}

#[derive(Clone)]
struct FileJob {
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
        disabled_providers: Arc::new(Mutex::new(HashSet::new())),
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
    for path in files {
        if state.workflow_cancelled().await {
            break;
        }
        file_tx.send(FileJob { path }).await?;
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
            .start_track(
                total,
                filename.clone(),
                format!(
                    "Matching {filename} · attempt {attempt}/{}",
                    cfg.track_attempts
                ),
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
    if cfg.acoustid_key().is_empty() || !cfg.metadata_sources.acoustid.enabled {
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
    let raw_candidate_count = candidates.len();
    dedupe_candidates(&mut candidates);
    sort_candidates_for_decision(&mut candidates);
    state
        .log(
            "info",
            "providers",
            Some(filename),
            &format!(
                "Providers returned {raw_candidate_count} raw candidate(s), {} usable candidate(s)",
                candidates.len()
            ),
        )
        .await;
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
        let message = "No provider candidate found; counted as unmatched";
        persist_unmatched(&state.pool, path, &info, &message).await?;
        state.log("warn", "match", Some(filename), &message).await;
        return Ok(false);
    };
    let second_score = comparable_second_score(&candidates);
    let has_safe_evidence = candidate_has_fingerprint(best) || candidate_source_count(best) >= 2;
    let auto_select = has_safe_evidence
        && crate::domain::matcher::auto_selectable_for_strategy(
            cfg.matching_strategy,
            best.score,
            second_score,
            best.duration_delta,
        );
    if !auto_select {
        state.increment_unmatched().await;
        let message = if best.score >= 70.0 {
            "Release uncertain; review required before applying metadata".to_owned()
        } else {
            "No candidate met strict matching rules; counted as unmatched".to_owned()
        };
        if best.score >= 70.0 {
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
    let primary_modes = [ProviderMode::Primary, ProviderMode::Parallel];
    let fallback_modes = [ProviderMode::Fallback];
    let all_modes = [
        ProviderMode::Primary,
        ProviderMode::Fallback,
        ProviderMode::Parallel,
    ];

    match cfg.matching_strategy {
        MatchingStrategy::Aggressive => {
            out.extend(
                query_candidate_modes(
                    state,
                    cfg,
                    limits,
                    fingerprint,
                    duration,
                    current,
                    filename,
                    &all_modes,
                )
                .await?,
            );
        }
        MatchingStrategy::Safe => {
            out.extend(
                query_candidate_modes(
                    state,
                    cfg,
                    limits,
                    fingerprint,
                    duration,
                    current,
                    filename,
                    &primary_modes,
                )
                .await?,
            );
            if out.is_empty() {
                state
                    .log(
                        "info",
                        "providers",
                        Some(filename),
                        "Primary sources returned no candidates; querying fallback sources",
                    )
                    .await;
                out.extend(
                    query_candidate_modes(
                        state,
                        cfg,
                        limits,
                        fingerprint,
                        duration,
                        current,
                        filename,
                        &fallback_modes,
                    )
                    .await?,
                );
            }
        }
        MatchingStrategy::Balanced => {
            out.extend(
                query_candidate_modes(
                    state,
                    cfg,
                    limits,
                    fingerprint,
                    duration,
                    current,
                    filename,
                    &primary_modes,
                )
                .await?,
            );
            let best = out
                .iter()
                .map(|candidate| candidate.score)
                .fold(0.0, f64::max);
            let has_strong_fingerprint = out
                .iter()
                .any(|candidate| candidate_has_fingerprint(candidate) && candidate.score >= 90.0);
            if !has_strong_fingerprint && (out.is_empty() || best < 85.0) {
                state
                    .log(
                        "info",
                        "providers",
                        Some(filename),
                        "Primary confidence is medium or empty; querying fallback sources",
                    )
                    .await;
                out.extend(
                    query_candidate_modes(
                        state,
                        cfg,
                        limits,
                        fingerprint,
                        duration,
                        current,
                        filename,
                        &fallback_modes,
                    )
                    .await?,
                );
            }
        }
    }
    let has_strong_fingerprint = out
        .iter()
        .any(|candidate| candidate_has_fingerprint(candidate) && candidate.score >= 90.0);
    if !has_strong_fingerprint {
        apply_enrichment_sources(state, cfg, limits, current, filename, &mut out).await;
    }
    apply_source_agreement(&mut out)?;
    if out.is_empty() {
        state
            .log(
                "warn",
                "providers",
                Some(filename),
                "No enabled metadata source returned candidates",
            )
            .await;
    }
    Ok(out)
}

fn sort_candidates_for_decision(candidates: &mut [providers::Candidate]) {
    candidates.sort_by(|a, b| {
        candidate_trust_tier(b)
            .cmp(&candidate_trust_tier(a))
            .then_with(|| b.score.total_cmp(&a.score))
    });
}

fn comparable_second_score(candidates: &[providers::Candidate]) -> Option<f64> {
    let best = candidates.first()?;
    let best_tier = candidate_trust_tier(best);
    candidates
        .iter()
        .skip(1)
        .find(|candidate| candidate_trust_tier(candidate) == best_tier)
        .map(|candidate| candidate.score)
}

fn candidate_trust_tier(candidate: &providers::Candidate) -> u8 {
    if candidate_has_fingerprint(candidate) {
        3
    } else if candidate.provider == "musicbrainz" {
        2
    } else if candidate_source_count(candidate) >= 2 {
        1
    } else {
        0
    }
}

async fn query_candidate_modes(
    state: &Arc<AppState>,
    cfg: &crate::config::Config,
    limits: &Arc<PipelineLimits>,
    fingerprint: &str,
    duration: f64,
    current: &audio::AudioInfo,
    filename: &str,
    modes: &[ProviderMode],
) -> Result<Vec<providers::Candidate>> {
    let mut out = Vec::new();
    if modes.contains(&cfg.metadata_sources.acoustid.mode) {
        out.extend(
            query_musicbrainz_acoustid(
                state,
                cfg,
                limits,
                fingerprint,
                duration,
                current,
                filename,
            )
            .await?,
        );
    }
    if modes.contains(&cfg.metadata_sources.musicbrainz.mode) {
        out.extend(query_musicbrainz_text(state, cfg, current, filename).await?);
    }
    if modes.contains(&cfg.metadata_sources.discogs.mode) {
        out.extend(query_discogs(state, cfg, limits, current, filename).await);
    }
    if modes.contains(&cfg.metadata_sources.lastfm.mode) {
        out.extend(query_lastfm(state, cfg, limits, current, filename).await);
    }
    if modes.contains(&cfg.metadata_sources.theaudiodb.mode) {
        out.extend(query_theaudiodb(state, cfg, limits, current, filename).await);
    }
    if modes.contains(&cfg.metadata_sources.wikidata.mode) {
        out.extend(query_wikidata(state, cfg, limits, current, filename).await);
    }
    Ok(out)
}

async fn query_musicbrainz_acoustid(
    state: &Arc<AppState>,
    cfg: &crate::config::Config,
    limits: &Arc<PipelineLimits>,
    fingerprint: &str,
    duration: f64,
    current: &audio::AudioInfo,
    filename: &str,
) -> Result<Vec<providers::Candidate>> {
    let mut out = Vec::new();
    if !cfg.metadata_sources.acoustid.enabled || !cfg.metadata_sources.musicbrainz.enabled {
        return Ok(out);
    }
    if cfg.acoustid_key().is_empty() {
        log_provider_skip(state, filename, "acoustid", "AcoustID API key is missing").await;
        return Ok(out);
    }
    let started = Instant::now();
    let hits = {
        let _permit = limits.acoustid.acquire().await?;
        providers::acoustid::lookup(
            &state.pool,
            &state.client,
            cfg.acoustid_key(),
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
            cfg.musicbrainz_user_agent(),
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
    Ok(out)
}

async fn query_musicbrainz_text(
    state: &Arc<AppState>,
    cfg: &crate::config::Config,
    current: &audio::AudioInfo,
    filename: &str,
) -> Result<Vec<providers::Candidate>> {
    let mut out = Vec::new();
    if !cfg.metadata_sources.musicbrainz.enabled {
        return Ok(out);
    }
    let Some(title) = current
        .title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(out);
    };
    let started = Instant::now();
    for mut candidate in providers::musicbrainz::search(
        &state.pool,
        &state.client,
        cfg.musicbrainz_user_agent(),
        title,
        current.artist.as_deref(),
    )
    .await?
    {
        score_text_candidate(&mut candidate, current, "musicbrainz_text_search")?;
        out.push(candidate);
    }
    state
        .log_entry(
            ActivityLogEntry::new(
                "info",
                "musicbrainz",
                format!("MusicBrainz tag search returned {} candidate(s)", out.len()),
            )
            .file(filename.to_owned())
            .duration_ms(started.elapsed().as_millis() as i64)
            .context(serde_json::json!({"title": title, "artist": current.artist})),
        )
        .await;
    Ok(out)
}

async fn query_discogs(
    state: &Arc<AppState>,
    cfg: &crate::config::Config,
    limits: &Arc<PipelineLimits>,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<providers::Candidate> {
    if !cfg.metadata_sources.discogs.enabled {
        return Vec::new();
    }
    if provider_disabled(limits, "discogs").await {
        return Vec::new();
    }
    let token = cfg
        .metadata_sources
        .discogs
        .token
        .as_str()
        .trim()
        .is_empty()
        .then_some(cfg.metadata_sources.discogs.api_key.as_str())
        .unwrap_or(cfg.metadata_sources.discogs.token.as_str());
    if token.trim().is_empty() {
        log_provider_skip(
            state,
            filename,
            "discogs",
            "Discogs token/API key is missing",
        )
        .await;
        return Vec::new();
    }
    let started = Instant::now();
    match providers::discogs::search(&state.pool, &state.client, Some(token), current).await {
        Ok(mut candidates) => {
            for candidate in &mut candidates {
                let _ = score_text_candidate(candidate, current, "discogs_release_search");
            }
            log_provider_count(state, filename, "discogs", candidates.len(), started).await;
            candidates
        }
        Err(error) => {
            handle_provider_error(state, limits, filename, "discogs", error).await;
            Vec::new()
        }
    }
}

async fn query_lastfm(
    state: &Arc<AppState>,
    cfg: &crate::config::Config,
    limits: &Arc<PipelineLimits>,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<providers::Candidate> {
    if !cfg.metadata_sources.lastfm.enabled {
        return Vec::new();
    }
    if provider_disabled(limits, "lastfm").await {
        return Vec::new();
    }
    let api_key = cfg.metadata_sources.lastfm.api_key.trim();
    if api_key.is_empty() {
        log_provider_skip(state, filename, "lastfm", "Last.fm API key is missing").await;
        return Vec::new();
    }
    let started = Instant::now();
    match providers::lastfm::search(&state.pool, &state.client, api_key, current).await {
        Ok(mut candidates) => {
            for candidate in &mut candidates {
                let _ = score_text_candidate(candidate, current, "lastfm_track_search");
            }
            log_provider_count(state, filename, "lastfm", candidates.len(), started).await;
            candidates
        }
        Err(error) => {
            handle_provider_error(state, limits, filename, "lastfm", error).await;
            Vec::new()
        }
    }
}

async fn query_theaudiodb(
    state: &Arc<AppState>,
    cfg: &crate::config::Config,
    limits: &Arc<PipelineLimits>,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<providers::Candidate> {
    if !cfg.metadata_sources.theaudiodb.enabled {
        return Vec::new();
    }
    if provider_disabled(limits, "theaudiodb").await {
        return Vec::new();
    }
    let api_key = cfg.metadata_sources.theaudiodb.api_key.trim();
    if api_key.is_empty() {
        log_provider_skip(
            state,
            filename,
            "theaudiodb",
            "TheAudioDB API key is missing",
        )
        .await;
        return Vec::new();
    }
    let started = Instant::now();
    match providers::theaudiodb::search(&state.pool, &state.client, api_key, current).await {
        Ok(mut candidates) => {
            for candidate in &mut candidates {
                let _ = score_text_candidate(candidate, current, "theaudiodb_track_search");
            }
            log_provider_count(state, filename, "theaudiodb", candidates.len(), started).await;
            candidates
        }
        Err(error) => {
            handle_provider_error(state, limits, filename, "theaudiodb", error).await;
            Vec::new()
        }
    }
}

async fn query_wikidata(
    state: &Arc<AppState>,
    cfg: &crate::config::Config,
    limits: &Arc<PipelineLimits>,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<providers::Candidate> {
    if !cfg.metadata_sources.wikidata.enabled {
        return Vec::new();
    }
    if provider_disabled(limits, "wikidata").await {
        return Vec::new();
    }
    let started = Instant::now();
    match providers::wikidata::search(&state.pool, &state.client, current).await {
        Ok(mut candidates) => {
            for candidate in &mut candidates {
                let _ = score_text_candidate(candidate, current, "wikidata_sparql");
            }
            log_provider_count(state, filename, "wikidata", candidates.len(), started).await;
            candidates
        }
        Err(error) => {
            handle_provider_error(state, limits, filename, "wikidata", error).await;
            Vec::new()
        }
    }
}

async fn apply_enrichment_sources(
    state: &Arc<AppState>,
    cfg: &crate::config::Config,
    limits: &Arc<PipelineLimits>,
    current: &audio::AudioInfo,
    filename: &str,
    candidates: &mut [providers::Candidate],
) {
    if candidates.is_empty() {
        return;
    }
    let mut enrichment = Vec::new();
    if cfg.metadata_sources.discogs.mode == ProviderMode::EnrichmentOnly {
        enrichment.extend(query_discogs(state, cfg, limits, current, filename).await);
    }
    if cfg.metadata_sources.lastfm.mode == ProviderMode::EnrichmentOnly {
        enrichment.extend(query_lastfm(state, cfg, limits, current, filename).await);
    }
    if cfg.metadata_sources.theaudiodb.mode == ProviderMode::EnrichmentOnly {
        enrichment.extend(query_theaudiodb(state, cfg, limits, current, filename).await);
    }
    if cfg.metadata_sources.wikidata.mode == ProviderMode::EnrichmentOnly {
        enrichment.extend(query_wikidata(state, cfg, limits, current, filename).await);
    }
    if enrichment.is_empty() {
        return;
    }
    for candidate in candidates {
        let mut sources = candidate_source_list(candidate);
        for evidence in &enrichment {
            if candidate_agrees(candidate, evidence) {
                sources.push(provider_display_name(&evidence.provider).to_owned());
                candidate.score = (candidate.score + 2.0).min(99.0);
                if candidate.genre.is_none() {
                    candidate.genre = evidence.genre.clone();
                }
                if candidate.cover_url.is_none() {
                    candidate.cover_url = evidence.cover_url.clone();
                }
            }
        }
        set_score_sources(candidate, sources).ok();
    }
}

fn score_text_candidate(
    candidate: &mut providers::Candidate,
    current: &audio::AudioInfo,
    source: &str,
) -> Result<()> {
    let title = current
        .title
        .as_deref()
        .map(|value| text_similarity(value, &candidate.title))
        .unwrap_or_default();
    let artist = current
        .artist
        .as_deref()
        .map(|value| text_similarity(value, &candidate.artist))
        .unwrap_or_default();
    let album_context = match (current.album.as_deref(), candidate.album.as_deref()) {
        (Some(left), Some(right)) => text_similarity(left, right),
        _ => 0.0,
    };
    let candidate_duration = candidate.duration_delta;
    let duration_delta = candidate_duration.map(|value| (current.duration - value).abs());
    let duration = duration_delta.map(duration_match).unwrap_or(0.0);
    candidate.duration_delta = duration_delta;
    let provider_cap = if candidate.provider == "musicbrainz" {
        82.0
    } else {
        78.0
    };
    let score = ((0.40 * title) + (0.30 * artist) + (0.15 * album_context) + (0.15 * duration))
        .clamp(0.0, 1.0)
        * 100.0;
    candidate.score = score.min(provider_cap);
    let mut value = serde_json::json!({
        "acoustid": 0.0,
        "duration": duration,
        "title": title,
        "artist": artist,
        "album_context": album_context,
        "provider_text_only": true,
        "auto_select_blocked": "Provider text match requires fingerprint evidence or multi-provider agreement",
        "final_score": candidate.score
    });
    value["source"] = serde_json::Value::String(source.to_owned());
    value["sources"] = serde_json::json!([provider_display_name(&candidate.provider)]);
    candidate.score_breakdown = Some(value.to_string());
    Ok(())
}

fn apply_source_agreement(candidates: &mut [providers::Candidate]) -> Result<()> {
    let snapshot = candidates.to_vec();
    for candidate in candidates {
        let mut sources = candidate_source_list(candidate);
        for other in &snapshot {
            if other.provider == candidate.provider {
                continue;
            }
            if candidate_agrees(candidate, other) {
                sources.push(provider_display_name(&other.provider).to_owned());
                let cap = if candidate_has_fingerprint(candidate) {
                    99.0
                } else {
                    88.0
                };
                candidate.score = (candidate.score + 6.0).min(cap);
            } else if text_close(&candidate.title, &other.title, 0.55)
                && !text_close(&candidate.artist, &other.artist, 0.55)
            {
                candidate.score = (candidate.score - 4.0).max(0.0);
            }
        }
        set_score_sources(candidate, sources)?;
    }
    Ok(())
}

fn dedupe_candidates(candidates: &mut Vec<providers::Candidate>) {
    candidates.sort_by(|a, b| b.score.total_cmp(&a.score));
    let mut seen = std::collections::HashSet::new();
    candidates.retain(|candidate| {
        let key = [
            normalize_match_key(&candidate.title),
            normalize_match_key(&candidate.artist),
            normalize_match_key(candidate.album.as_deref().unwrap_or_default()),
            normalize_match_key(candidate.release_id.as_deref().unwrap_or_default()),
        ]
        .join("|");
        seen.insert(key)
    });
}

fn normalize_match_key(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_ascii_alphanumeric())
        .collect::<String>()
        .to_ascii_lowercase()
}

fn candidate_has_fingerprint(candidate: &providers::Candidate) -> bool {
    candidate
        .score_breakdown
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .and_then(|value| value["acoustid"].as_f64())
        .is_some_and(|value| value > 0.0)
}

fn candidate_source_count(candidate: &providers::Candidate) -> usize {
    candidate_source_list(candidate).len()
}

fn text_similarity(left: &str, right: &str) -> f64 {
    strsim::normalized_levenshtein(&left.to_ascii_lowercase(), &right.to_ascii_lowercase())
}

fn duration_match(delta: f64) -> f64 {
    if delta <= 3.0 {
        1.0
    } else if delta <= 8.0 {
        0.65
    } else if delta <= 15.0 {
        0.3
    } else {
        0.0
    }
}

fn candidate_agrees(left: &providers::Candidate, right: &providers::Candidate) -> bool {
    text_close(&left.title, &right.title, 0.82)
        && text_close(&left.artist, &right.artist, 0.75)
        && match (left.album.as_deref(), right.album.as_deref()) {
            (Some(left_album), Some(right_album)) => text_close(left_album, right_album, 0.65),
            _ => true,
        }
}

fn text_close(left: &str, right: &str, threshold: f64) -> bool {
    if left.trim().is_empty() || right.trim().is_empty() {
        return false;
    }
    strsim::normalized_levenshtein(&left.to_ascii_lowercase(), &right.to_ascii_lowercase())
        >= threshold
}

fn candidate_source_list(candidate: &providers::Candidate) -> Vec<String> {
    let mut out = vec![provider_display_name(&candidate.provider).to_owned()];
    if let Some(value) = candidate
        .score_breakdown
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        && let Some(sources) = value["sources"].as_array()
    {
        out.extend(
            sources
                .iter()
                .filter_map(|source| source.as_str().map(str::to_owned)),
        );
    }
    out.sort();
    out.dedup();
    out
}

fn set_score_sources(candidate: &mut providers::Candidate, sources: Vec<String>) -> Result<()> {
    let mut value = candidate
        .score_breakdown
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    value["sources"] = serde_json::to_value(sources)?;
    value["source_agreement"] = serde_json::json!(value["sources"].as_array().map_or(0, Vec::len));
    value["final_score"] = serde_json::json!(candidate.score);
    candidate.score_breakdown = Some(value.to_string());
    Ok(())
}

async fn log_provider_count(
    state: &Arc<AppState>,
    filename: &str,
    provider: &str,
    count: usize,
    started: Instant,
) {
    state
        .log_entry(
            ActivityLogEntry::new(
                "info",
                provider,
                format!("{provider} returned {count} candidate(s)"),
            )
            .file(filename.to_owned())
            .duration_ms(started.elapsed().as_millis() as i64),
        )
        .await;
}

async fn log_provider_skip(state: &Arc<AppState>, filename: &str, provider: &str, reason: &str) {
    state.log("warn", provider, Some(filename), reason).await;
}

async fn log_provider_error(
    state: &Arc<AppState>,
    filename: &str,
    provider: &str,
    error: anyhow::Error,
) {
    state
        .log_entry(
            ActivityLogEntry::new("error", provider, "Provider lookup failed")
                .file(filename.to_owned())
                .error_text(format!("{error:#}")),
        )
        .await;
}

async fn provider_disabled(limits: &Arc<PipelineLimits>, provider: &str) -> bool {
    limits.disabled_providers.lock().await.contains(provider)
}

async fn disable_provider(limits: &Arc<PipelineLimits>, provider: &str) -> bool {
    limits
        .disabled_providers
        .lock()
        .await
        .insert(provider.to_owned())
}

async fn handle_provider_error(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    filename: &str,
    provider: &str,
    error: anyhow::Error,
) {
    let error_text = format!("{error:#}");
    let lower = error_text.to_ascii_lowercase();
    let disable_reason = if lower.contains("401 unauthorized") {
        Some("authentication failed; check the configured API key/token")
    } else if lower.contains("429 too many requests") {
        Some("rate limited by provider")
    } else if lower.contains("504 gateway timeout") || lower.contains("timed out") {
        Some("provider timed out")
    } else {
        None
    };

    if let Some(reason) = disable_reason
        && disable_provider(limits, provider).await
    {
        state
            .log_entry(
                ActivityLogEntry::new(
                    "warn",
                    provider,
                    format!("Provider disabled for this scan: {reason}"),
                )
                .file(filename.to_owned())
                .error_text(error_text),
            )
            .await;
        return;
    }

    log_provider_error(state, filename, provider, error).await;
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
    let candidate_id = sqlx::query("INSERT INTO candidates(track_id,provider,title,artist,album,album_artist,track_number,track_total,disc_number,disc_total,year,genre,composer,label,isrc,cover_url,musicbrainz_recording_id,musicbrainz_release_id,release_country,release_date,release_type,release_secondary_types,is_compilation,duration_delta,score_breakdown,musicbrainz_artist_id,musicbrainz_album_artist_id,score,raw_json) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(track_id)
        .bind(if c.provider.is_empty() {
            "musicbrainz"
        } else {
            c.provider.as_str()
        })
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
        .last_insert_rowid();
    insert_candidate_source(
        tx,
        candidate_id,
        provider_display_name(if c.provider.is_empty() {
            "musicbrainz"
        } else {
            c.provider.as_str()
        }),
        Some(c.score),
        serde_json::json!({
            "reason": provider_reason(if c.provider.is_empty() { "musicbrainz" } else { c.provider.as_str() }),
            "recording_id": c.recording_id,
            "release_id": c.release_id
        }),
        Some(&c.raw_json),
    )
    .await?;
    if let Some(score_breakdown) = &c.score_breakdown
        && score_breakdown.contains("\"acoustid\"")
    {
        insert_candidate_source(
            tx,
            candidate_id,
            "AcoustID",
            Some(c.score),
            serde_json::json!({
                "reason": "Fingerprint evidence contributed to score",
                "breakdown": score_breakdown
            }),
            None,
        )
        .await?;
    }
    Ok(candidate_id)
}

fn provider_display_name(provider: &str) -> &str {
    match provider {
        "acoustid" => "AcoustID",
        "discogs" => "Discogs",
        "lastfm" => "Last.fm",
        "theaudiodb" => "TheAudioDB",
        "wikidata" => "Wikidata",
        _ => "MusicBrainz",
    }
}

fn provider_reason(provider: &str) -> &str {
    match provider {
        "discogs" => "Release, label, catalog, and physical media metadata",
        "lastfm" => "Track popularity, tags, and MusicBrainz ID evidence",
        "theaudiodb" => "Track, album, genre, and image enrichment",
        "wikidata" => "Structured identifier and external-link evidence",
        _ => "Canonical recording and release metadata",
    }
}

async fn insert_candidate_source(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    candidate_id: i64,
    provider: &str,
    confidence: Option<f64>,
    reason: serde_json::Value,
    raw_json: Option<&str>,
) -> Result<()> {
    sqlx::query("INSERT INTO candidate_sources(candidate_id,provider,confidence,reason_json,raw_json,created_at) VALUES(?,?,?,?,?,?)")
        .bind(candidate_id)
        .bind(provider)
        .bind(confidence)
        .bind(reason.to_string())
        .bind(raw_json)
        .bind(Utc::now().to_rfc3339())
        .execute(&mut **tx)
        .await?;
    Ok(())
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
        let candidates = vec![providers::Candidate {
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
            providers::Candidate {
                provider: "discogs".into(),
                title: "Song".into(),
                artist: "Artist".into(),
                album: Some("Album".into()),
                score: 82.0,
                score_breakdown: Some(serde_json::json!({"sources":["Discogs"]}).to_string()),
                ..Default::default()
            },
            providers::Candidate {
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
    fn provider_only_candidate_without_agreement_is_not_safe_auto_select() {
        let candidate = providers::Candidate {
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
        assert!(crate::domain::matcher::auto_selectable_for_strategy(
            crate::types::MatchingStrategy::Aggressive,
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
            providers::Candidate {
                provider: "lastfm".into(),
                title: "Out of Time".into(),
                artist: "The Weeknd".into(),
                score: 96.0,
                duration_delta: Some(1.0),
                score_breakdown: Some(serde_json::json!({"sources":["Last.fm"]}).to_string()),
                ..Default::default()
            },
            providers::Candidate {
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
        assert_eq!(comparable_second_score(&candidates), None);
        assert!(candidate_has_fingerprint(&candidates[0]));
        assert!(crate::domain::matcher::auto_selectable_for_strategy(
            crate::types::MatchingStrategy::Balanced,
            candidates[0].score,
            comparable_second_score(&candidates),
            candidates[0].duration_delta
        ));
    }

    #[test]
    fn dedupe_candidates_keeps_strongest_duplicate() {
        let mut candidates = vec![
            providers::Candidate {
                provider: "lastfm".into(),
                title: "Song!".into(),
                artist: "Artist".into(),
                album: Some("Album".into()),
                score: 72.0,
                ..Default::default()
            },
            providers::Candidate {
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
        let candidates = vec![providers::Candidate {
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
}
