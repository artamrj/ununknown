use crate::{
    app::{ActivityLogEntry, AppState},
    domain::audio,
    infrastructure::{fingerprint_cache, media::fingerprint, providers},
    types::WorkflowPhase,
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

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
enum ProcessOutcome {
    Matched,
    NeedsReview,
    Corrupt,
}

#[derive(Clone, Copy)]
struct FingerprintEvidence<'a> {
    value: &'a str,
    duration: f64,
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
        metadata: Arc::new(Semaphore::new(cfg.scan_workers)),
        fingerprint: Arc::new(Semaphore::new(cfg.fingerprint_workers)),
        acoustid: Arc::new(Semaphore::new(cfg.lookup_workers)),
        disabled_providers: Arc::new(Mutex::new(HashSet::new())),
    });
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
    let filename = job
        .path
        .file_name()
        .and_then(|v| v.to_str())
        .unwrap_or("audio")
        .to_owned();
    const ATTEMPTS: usize = 2;
    for attempt in 1..=ATTEMPTS {
        if state.workflow_cancelled().await {
            return;
        }
        state
            .start_track(
                total,
                filename.clone(),
                format!("Matching {filename} · attempt {attempt}/{}", ATTEMPTS),
            )
            .await;
        match process(&state, &limits, &persist_tx, &job.path).await {
            Ok(ProcessOutcome::Matched) => {
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
            Ok(ProcessOutcome::NeedsReview) => {
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
            Ok(ProcessOutcome::Corrupt) => {
                state
                    .log(
                        "error",
                        "integrity",
                        Some(&filename),
                        "Damaged audio was blocked from metadata writing",
                    )
                    .await;
                break;
            }
            Err(error) if attempt < ATTEMPTS => {
                tracing::warn!(path=%job.path.display(), attempt, "track attempt failed: {error:#}");
                state
                    .log_entry(
                        ActivityLogEntry::new("warn", "fetch", "Track attempt failed; retrying")
                            .file(filename.clone())
                            .attempt(attempt as i64)
                            .error_text(format!("{error:#}"))
                            .context(serde_json::json!({
                                "path": job.path.display().to_string(),
                                "max_attempts": ATTEMPTS
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
                                "max_attempts": ATTEMPTS
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
) -> Result<ProcessOutcome> {
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
            "integrity",
            Some(filename),
            "Decoding audio to check file integrity",
        )
        .await;
    let integrity_started = Instant::now();
    match crate::infrastructure::media::integrity::check(&state.pool, path).await {
        Ok(crate::infrastructure::media::integrity::Integrity::Healthy) => {
            state
                .log_entry(
                    ActivityLogEntry::new("ok", "integrity", "Audio integrity check passed")
                        .file(filename.to_owned())
                        .duration_ms(integrity_started.elapsed().as_millis() as i64),
                )
                .await;
        }
        Ok(crate::infrastructure::media::integrity::Integrity::Corrupt(diagnostic)) => {
            state.increment_failed().await;
            persist_corrupt(&state.pool, path, &info, &diagnostic).await?;
            state
                .log_entry(
                    ActivityLogEntry::new("error", "integrity", "Audio file is damaged")
                        .file(filename.to_owned())
                        .error_text(diagnostic),
                )
                .await;
            return Ok(ProcessOutcome::Corrupt);
        }
        Err(error) => {
            state
                .log_entry(
                    ActivityLogEntry::new(
                        "warn",
                        "integrity",
                        "Integrity check unavailable; continuing metadata matching",
                    )
                    .file(filename.to_owned())
                    .error_text(format!("{error:#}")),
                )
                .await;
        }
    }
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
        .await
    };
    let (fp, duration) = match fingerprint_result {
        Ok(result) => {
            let message = match result.source {
                fingerprint_cache::FingerprintSource::Cache => "Fingerprint reused from cache",
                fingerprint_cache::FingerprintSource::Generated => "Fingerprint generated",
            };
            state
                .log_entry(
                    ActivityLogEntry::new("ok", "fingerprint", message)
                        .file(filename.to_owned())
                        .duration_ms(started.elapsed().as_millis() as i64),
                )
                .await;
            (result.fingerprint, result.duration)
        }
        Err(error) => {
            state
                .log_entry(
                    ActivityLogEntry::new(
                        "warn",
                        "fingerprint",
                        "Fingerprint unavailable; continuing with text and web sources",
                    )
                    .file(filename.to_owned())
                    .error_text(format!("{error:#}")),
                )
                .await;
            (String::new(), info.duration)
        }
    };
    let cfg = state.config.read().await.clone();
    if cfg.acoustid_key.is_empty() {
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
    let mut candidates = identify(
        state,
        &cfg,
        limits,
        path,
        FingerprintEvidence {
            value: &fp,
            duration,
        },
        &info,
        filename,
    )
    .await?;
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
    let Some(best) = candidates.first() else {
        state.increment_unmatched().await;
        let fingerprint_note = if fp.is_empty() {
            " Fingerprint creation failed; install Chromaprint (fpcalc) to identify difficult tracks."
        } else if cfg.acoustid_key.is_empty() {
            " A fingerprint was created, but online fingerprint lookup needs an AcoustID API key."
        } else {
            ""
        };
        let message = format!(
            "No catalog match from Apple Music, Deezer, MusicBrainz, or the enabled optional sources.{fingerprint_note}"
        );
        persist_unmatched(&state.pool, path, &info, &message).await?;
        state.log("warn", "match", Some(filename), &message).await;
        return Ok(ProcessOutcome::NeedsReview);
    };
    let second_score = comparable_second_score(&candidates);
    let has_safe_evidence = candidate_has_fingerprint(best) || candidate_source_count(best) >= 2;
    let exact_unique = unique_exact_catalog_match(best, &candidates, &info);
    let auto_select = exact_unique
        || (has_safe_evidence
            && crate::domain::matcher::auto_selectable(
                best.score,
                second_score,
                best.duration_delta,
            ));
    if !auto_select {
        state.increment_unmatched().await;
        let message = if best.score >= 40.0 {
            let mut source_names = candidates
                .iter()
                .flat_map(candidate_source_list)
                .collect::<HashSet<_>>()
                .into_iter()
                .collect::<Vec<_>>();
            source_names.sort();
            let sources = source_names.join(", ");
            let title_match = info
                .title
                .as_deref()
                .is_some_and(|title| title_similarity(title, &best.title) >= 0.94);
            let artist_match = info
                .artist
                .as_deref()
                .is_some_and(|artist| artist_similarity(artist, &best.artist) >= 0.75);
            match (
                candidates.len(),
                info.album.as_deref(),
                best.album.as_deref(),
            ) {
                _ if title_match
                    && artist_match
                    && best.duration_delta.is_some_and(|delta| delta > 15.0) =>
                {
                    format!(
                        "Found matching catalog metadata, but this audio is {:.0} seconds longer or shorter; it may be a music-video, live, or edited version. Review before applying it.",
                        best.duration_delta.unwrap_or_default()
                    )
                }
                _ if title_match && !artist_match => format!(
                    "Found this title in {sources}, but only by different performers; keep the current artist or enter this performance manually."
                ),
                (1, Some(existing), Some(found)) if text_similarity(existing, found) < 0.65 => {
                    format!(
                        "Found one close match from {sources}, but its album “{found}” conflicts with the existing album “{existing}”; review before applying it."
                    )
                }
                _ => format!(
                    "Found {} possible release(s) from {sources}, but the release is ambiguous; choose the correct one.",
                    candidates.len()
                ),
            }
        } else {
            "No candidate met strict matching rules; counted as unmatched".to_owned()
        };
        if best.score >= 40.0 {
            persist_review(&state.pool, path, &info, &candidates, &message).await?;
        } else {
            persist_unmatched(&state.pool, path, &info, &message).await?;
        }
        state.log("warn", "match", Some(filename), &message).await;
        return Ok(ProcessOutcome::NeedsReview);
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
    Ok(ProcessOutcome::Matched)
}

async fn identify(
    state: &Arc<AppState>,
    cfg: &crate::config::Config,
    limits: &Arc<PipelineLimits>,
    path: &Path,
    fingerprint: FingerprintEvidence<'_>,
    current: &audio::AudioInfo,
    filename: &str,
) -> Result<Vec<providers::Candidate>> {
    let mut out = match query_musicbrainz_acoustid(
        state,
        cfg,
        limits,
        fingerprint.value,
        fingerprint.duration,
        current,
        filename,
    )
    .await
    {
        Ok(candidates) => candidates,
        Err(error) => {
            handle_provider_error(state, limits, filename, "acoustid", error).await;
            Vec::new()
        }
    };
    let acoustid_matched = !out.is_empty();
    if !cfg.youtube_api_key.trim().is_empty() {
        out.extend(query_youtube(state, limits, &cfg.youtube_api_key, current, filename).await);
    }
    match query_musicbrainz_text(state, cfg, current, filename).await {
        Ok(candidates) => out.extend(candidates),
        Err(error) => handle_provider_error(state, limits, filename, "musicbrainz", error).await,
    }
    match query_itunes(state, cfg, current, filename).await {
        Ok(candidates) => out.extend(candidates),
        Err(error) => handle_provider_error(state, limits, filename, "itunes", error).await,
    }
    out.extend(query_deezer(state, limits, current, filename).await);
    let has_strong_free_candidate = out.iter().any(|candidate| candidate.score >= 90.0);
    if !acoustid_matched
        && !has_strong_free_candidate
        && !cfg.audd_token.trim().is_empty()
        && !fingerprint.value.is_empty()
    {
        let audd_candidates = query_audd(
            state,
            limits,
            &cfg.audd_token,
            path,
            fingerprint.value,
            current,
            filename,
        )
        .await;
        if let Some(recognized) = audd_candidates.first() {
            let recognized_info = audio::AudioInfo {
                title: Some(recognized.title.clone()),
                artist: Some(recognized.artist.clone()),
                album: recognized.album.clone(),
                album_artist: recognized.album_artist.clone(),
                duration: current.duration,
                format: current.format.clone(),
                ..Default::default()
            };
            match query_itunes(state, cfg, &recognized_info, filename).await {
                Ok(candidates) => out.extend(candidates),
                Err(error) => handle_provider_error(state, limits, filename, "itunes", error).await,
            }
            out.extend(query_deezer(state, limits, &recognized_info, filename).await);
        }
        out.extend(audd_candidates);
    }
    if !cfg.spotify_client_id.trim().is_empty() && !cfg.spotify_client_secret.trim().is_empty() {
        let isrcs = out
            .iter()
            .filter_map(|candidate| candidate.isrc.clone())
            .collect::<Vec<_>>();
        out.extend(query_spotify(state, limits, cfg, current, filename, &isrcs).await);
    }
    if !cfg.soundcloud_client_id.trim().is_empty()
        && !cfg.soundcloud_client_secret.trim().is_empty()
    {
        out.extend(query_soundcloud(state, limits, cfg, current, filename).await);
    }
    out.extend(query_discogs(state, cfg, limits, current, filename).await);
    out.extend(query_lastfm(state, cfg, limits, current, filename).await);
    out.extend(query_theaudiodb(state, cfg, limits, current, filename).await);
    out.extend(query_wikidata(state, limits, current, filename).await);
    for candidate in &mut out {
        normalize_candidate_credits(candidate);
    }
    apply_source_agreement(&mut out)?;
    enrich_artwork_fallbacks(&mut out)?;
    apply_artwork_override(&state.pool, path, &mut out).await?;
    let artist_genres = if crate::domain::genre::needs_artist_lookup(&out, current) {
        if let Some(artist) = current.artist.as_deref() {
            match providers::wikidata::artist_genres(&state.pool, &state.client, artist).await {
                Ok(genres) => genres,
                Err(error) => {
                    state
                        .log_entry(
                            ActivityLogEntry::new(
                                "warn",
                                "genre",
                                "Artist genre enrichment failed; using track evidence",
                            )
                            .file(filename.to_owned())
                            .error(error.as_ref()),
                        )
                        .await;
                    Vec::new()
                }
            }
        } else {
            Vec::new()
        }
    } else {
        Vec::new()
    };
    crate::domain::genre::enrich(&mut out, current, &artist_genres)?;
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
    } else if matches!(
        candidate.provider.as_str(),
        "musicbrainz" | "itunes" | "deezer" | "spotify"
    ) {
        2
    } else if candidate_source_count(candidate) >= 2 {
        1
    } else {
        0
    }
}

async fn query_audd(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    token: &str,
    path: &Path,
    fingerprint: &str,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<providers::Candidate> {
    if provider_disabled(limits, "audd").await {
        return Vec::new();
    }
    let started = Instant::now();
    match providers::audd::recognize(
        &state.pool,
        &state.client,
        token,
        path,
        fingerprint,
        current.duration,
    )
    .await
    {
        Ok(mut candidates) => {
            for candidate in &mut candidates {
                normalize_candidate_credits(candidate);
                candidate.duration_delta = candidate
                    .duration_delta
                    .map(|candidate_duration| (current.duration - candidate_duration).abs());
                if let Some(raw) = candidate.score_breakdown.as_deref()
                    && let Ok(mut value) = serde_json::from_str::<serde_json::Value>(raw)
                {
                    value["duration_delta"] = serde_json::json!(candidate.duration_delta);
                    candidate.score_breakdown = Some(value.to_string());
                }
            }
            log_provider_count(state, filename, "audd", candidates.len(), started).await;
            candidates
        }
        Err(error) => {
            handle_provider_error(state, limits, filename, "audd", error).await;
            Vec::new()
        }
    }
}

async fn query_youtube(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    api_key: &str,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<providers::Candidate> {
    if provider_disabled(limits, "youtube").await {
        return Vec::new();
    }
    let started = Instant::now();
    match providers::youtube::lookup_filename_id(&state.pool, &state.client, api_key, filename)
        .await
    {
        Ok(mut candidates) => {
            for candidate in &mut candidates {
                let _ = score_text_candidate(candidate, current, "youtube_exact_video_id");
            }
            log_provider_count(state, filename, "youtube", candidates.len(), started).await;
            candidates
        }
        Err(error) => {
            handle_provider_error(state, limits, filename, "youtube", error).await;
            Vec::new()
        }
    }
}

async fn query_spotify(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    cfg: &crate::config::Config,
    current: &audio::AudioInfo,
    filename: &str,
    isrcs: &[String],
) -> Vec<providers::Candidate> {
    if provider_disabled(limits, "spotify").await {
        return Vec::new();
    }
    let Some(title) = current
        .title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
    else {
        return Vec::new();
    };
    let started = Instant::now();
    match providers::spotify::search(
        &state.client,
        &state.spotify_auth,
        &cfg.spotify_client_id,
        &cfg.spotify_client_secret,
        title,
        current.artist.as_deref(),
        isrcs,
    )
    .await
    {
        Ok(mut candidates) => {
            for candidate in &mut candidates {
                let identifier_match = candidate.isrc.as_deref().is_some_and(|candidate_isrc| {
                    isrcs
                        .iter()
                        .any(|isrc| isrc.eq_ignore_ascii_case(candidate_isrc))
                });
                let _ = score_text_candidate(candidate, current, "spotify_catalog_search");
                if identifier_match {
                    candidate.score = candidate.score.max(96.0);
                    if let Some(raw) = candidate.score_breakdown.as_deref()
                        && let Ok(mut value) = serde_json::from_str::<serde_json::Value>(raw)
                    {
                        value["identifier_match"] = serde_json::json!("isrc");
                        value["final_score"] = serde_json::json!(candidate.score);
                        candidate.score_breakdown = Some(value.to_string());
                    }
                }
            }
            log_provider_count(state, filename, "spotify", candidates.len(), started).await;
            candidates
        }
        Err(error) => {
            handle_provider_error(state, limits, filename, "spotify", error).await;
            Vec::new()
        }
    }
}

async fn query_soundcloud(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    cfg: &crate::config::Config,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<providers::Candidate> {
    if provider_disabled(limits, "soundcloud").await {
        return Vec::new();
    }
    let Some(title) = current
        .title
        .as_deref()
        .filter(|title| !title.trim().is_empty())
    else {
        return Vec::new();
    };
    let started = Instant::now();
    match providers::soundcloud::search(
        &state.client,
        &state.soundcloud_auth,
        &cfg.soundcloud_client_id,
        &cfg.soundcloud_client_secret,
        title,
        current.artist.as_deref(),
    )
    .await
    {
        Ok(mut candidates) => {
            for candidate in &mut candidates {
                let _ = score_text_candidate(candidate, current, "soundcloud_track_search");
            }
            log_provider_count(state, filename, "soundcloud", candidates.len(), started).await;
            candidates
        }
        Err(error) => {
            handle_provider_error(state, limits, filename, "soundcloud", error).await;
            Vec::new()
        }
    }
}

async fn query_itunes(
    state: &Arc<AppState>,
    cfg: &crate::config::Config,
    current: &audio::AudioInfo,
    filename: &str,
) -> Result<Vec<providers::Candidate>> {
    let Some(title) = current
        .title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(Vec::new());
    };
    let started = Instant::now();
    let mut candidates = providers::itunes::search(
        &state.pool,
        &state.client,
        title,
        current.artist.as_deref(),
        current.album.as_deref(),
    )
    .await?;
    for candidate in &mut candidates {
        score_text_candidate(candidate, current, "itunes_catalog_search")?;
    }
    let needs_alias_search = current
        .artist
        .as_deref()
        .is_some_and(|artist| !artist.is_ascii())
        && candidates.iter().all(|candidate| candidate.score < 70.0);
    if needs_alias_search && let Some(artist) = current.artist.as_deref() {
        let aliases = providers::musicbrainz::artist_aliases(
            &state.pool,
            &state.client,
            &cfg.musicbrainz_user_agent,
            artist,
        )
        .await
        .unwrap_or_default();
        for alias in aliases {
            let mut alias_info = current.clone();
            alias_info.artist = Some(alias.clone());
            let Ok(mut alias_candidates) =
                providers::itunes::search(&state.pool, &state.client, "", Some(&alias), None).await
            else {
                continue;
            };
            for candidate in &mut alias_candidates {
                score_text_candidate(candidate, &alias_info, "itunes_artist_alias_search")?;
            }
            candidates.extend(alias_candidates);
        }
    }
    log_provider_count(state, filename, "itunes", candidates.len(), started).await;
    Ok(candidates)
}

async fn query_deezer(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<providers::Candidate> {
    if provider_disabled(limits, "deezer").await {
        return Vec::new();
    }
    let Some(title) = current
        .title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return Vec::new();
    };
    let started = Instant::now();
    match providers::deezer::search(&state.pool, &state.client, title, current.artist.as_deref())
        .await
    {
        Ok(mut candidates) => {
            for candidate in &mut candidates {
                let _ = score_text_candidate(candidate, current, "deezer_catalog_search");
            }
            log_provider_count(state, filename, "deezer", candidates.len(), started).await;
            candidates
        }
        Err(error) => {
            handle_provider_error(state, limits, filename, "deezer", error).await;
            Vec::new()
        }
    }
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
    if cfg.acoustid_key.is_empty() || fingerprint.is_empty() {
        log_provider_skip(state, filename, "acoustid", "AcoustID API key is missing").await;
        return Ok(out);
    }
    let started = Instant::now();
    let hits = {
        let _permit = limits.acoustid.acquire().await?;
        providers::acoustid::lookup(
            &state.pool,
            &state.client,
            &cfg.acoustid_key,
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
        &cfg.musicbrainz_user_agent,
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
    if provider_disabled(limits, "discogs").await {
        return Vec::new();
    }
    let token = cfg.discogs_token.trim();
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
    if provider_disabled(limits, "lastfm").await {
        return Vec::new();
    }
    let api_key = cfg.lastfm_key.trim();
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
    if provider_disabled(limits, "theaudiodb").await {
        return Vec::new();
    }
    let api_key = cfg.theaudiodb_key.trim();
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
    limits: &Arc<PipelineLimits>,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<providers::Candidate> {
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

fn score_text_candidate(
    candidate: &mut providers::Candidate,
    current: &audio::AudioInfo,
    source: &str,
) -> Result<()> {
    normalize_candidate_credits(candidate);
    preserve_album_context_for_catalog_single(candidate, current);
    let title = current
        .title
        .as_deref()
        .map(|value| title_similarity(value, &candidate.title))
        .unwrap_or_default();
    let artist = current
        .artist
        .as_deref()
        .map(|value| artist_similarity(value, &candidate.artist))
        .unwrap_or_default();
    let album_context = match (current.album.as_deref(), candidate.album.as_deref()) {
        (Some(left), Some(right)) => text_similarity(left, right),
        _ => 0.0,
    };
    let candidate_duration = candidate.duration_delta;
    let duration_delta = candidate_duration.map(|value| (current.duration - value).abs());
    let duration = duration_delta.map(duration_match).unwrap_or(0.0);
    candidate.duration_delta = duration_delta;
    let provider_cap = match candidate.provider.as_str() {
        "itunes" => 94.0,
        "deezer" => 90.0,
        "musicbrainz" => 82.0,
        _ => 78.0,
    };
    let has_album_context = current
        .album
        .as_deref()
        .is_some_and(|album| !album.trim().is_empty() && !album.trim().starts_with('@'))
        && candidate.album.is_some();
    let score = if has_album_context {
        (0.35 * title) + (0.25 * artist) + (0.25 * album_context) + (0.15 * duration)
    } else if current
        .artist
        .as_deref()
        .is_some_and(|value| !value.trim().is_empty())
    {
        (0.45 * title) + (0.35 * artist) + (0.20 * duration)
    } else {
        (0.75 * title) + (0.25 * duration)
    }
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
        "auto_select_rule": "Text-only matches require an exact unique title, artist, and duration or independent source agreement",
        "final_score": candidate.score
    });
    value["source"] = serde_json::Value::String(source.to_owned());
    value["sources"] = serde_json::json!([provider_display_name(&candidate.provider)]);
    candidate.score_breakdown = Some(value.to_string());
    Ok(())
}

fn preserve_album_context_for_catalog_single(
    candidate: &mut providers::Candidate,
    current: &audio::AudioInfo,
) {
    let Some(current_album) = current
        .album
        .as_deref()
        .filter(|album| !album.trim().is_empty() && !album.trim().starts_with('@'))
    else {
        return;
    };
    let is_catalog_single = candidate
        .album
        .as_deref()
        .is_some_and(|album| album.to_lowercase().contains("single"));
    let album_track = current.track_number.is_some_and(|number| number > 1);
    let duration_close = candidate
        .duration_delta
        .is_some_and(|duration| (current.duration - duration).abs() <= 5.0);
    let text_exact = current
        .title
        .as_deref()
        .is_some_and(|title| title_similarity(title, &candidate.title) >= 0.94)
        && current
            .artist
            .as_deref()
            .is_some_and(|artist| artist_similarity(artist, &candidate.artist) >= 0.90);
    if is_catalog_single && album_track && duration_close && text_exact {
        candidate.album = Some(current_album.to_owned());
        candidate.track_number = current.track_number.map(i64::from);
        candidate.track_total = None;
        candidate.release_id = None;
    }
}

fn apply_source_agreement(candidates: &mut [providers::Candidate]) -> Result<()> {
    let snapshot = candidates.to_vec();
    for candidate in candidates {
        let mut sources = candidate_source_list(candidate);
        let mut agreeing_providers = HashSet::new();
        let mut disagreeing_providers = HashSet::new();
        for other in &snapshot {
            if other.provider == candidate.provider {
                continue;
            }
            if candidate_agrees(candidate, other) {
                sources.push(provider_display_name(&other.provider).to_owned());
                if agreeing_providers.insert(other.provider.as_str()) {
                    let cap = if candidate_has_fingerprint(candidate) {
                        99.0
                    } else {
                        98.0
                    };
                    candidate.score = (candidate.score + 6.0).min(cap);
                }
            } else if text_close(&candidate.title, &other.title, 0.55)
                && !text_close(&candidate.artist, &other.artist, 0.55)
                && disagreeing_providers.insert(other.provider.as_str())
            {
                candidate.score = (candidate.score - 4.0).max(0.0);
            }
        }
        set_score_sources(candidate, sources)?;
    }
    Ok(())
}

fn normalize_candidate_credits(candidate: &mut providers::Candidate) {
    candidate.artist = crate::domain::credits::prefer_latin_alias(&candidate.artist);
    candidate.album_artist = candidate
        .album_artist
        .as_deref()
        .map(crate::domain::credits::prefer_latin_alias);
    let credits = crate::domain::credits::normalize_featured(&candidate.artist, &candidate.title);
    candidate.artist = credits.artist;
    candidate.title = credits.title;
}

fn enrich_artwork_fallbacks(candidates: &mut [providers::Candidate]) -> Result<()> {
    let snapshot = candidates.to_vec();
    for candidate in candidates {
        let mut artwork = snapshot
            .iter()
            .filter(|other| artwork_agrees(candidate, other))
            .filter_map(|other| {
                other.cover_url.as_deref().map(|url| {
                    (
                        artwork_provider_priority(&other.provider),
                        other.score,
                        other.provider.clone(),
                        url.to_owned(),
                    )
                })
            })
            .collect::<Vec<_>>();
        artwork.sort_by(|left, right| {
            right
                .0
                .cmp(&left.0)
                .then_with(|| right.1.total_cmp(&left.1))
        });
        artwork.dedup_by(|left, right| left.3 == right.3);
        candidate.cover_url = artwork
            .first()
            .map(|item| item.3.clone())
            .or_else(|| candidate.cover_url.clone());
        if artwork.is_empty() {
            continue;
        }
        let mut breakdown = candidate
            .score_breakdown
            .as_deref()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        breakdown["artwork_candidates"] = serde_json::Value::Array(
            artwork
                .into_iter()
                .map(|(_, _, provider, url)| {
                    serde_json::json!({
                        "provider": provider_display_name(&provider),
                        "url": url
                    })
                })
                .collect(),
        );
        candidate.score_breakdown = Some(breakdown.to_string());
    }
    Ok(())
}

fn artwork_agrees(left: &providers::Candidate, right: &providers::Candidate) -> bool {
    if !candidate_agrees(left, right) {
        return false;
    }
    if left
        .isrc
        .as_deref()
        .zip(right.isrc.as_deref())
        .is_some_and(|(left_isrc, right_isrc)| left_isrc.eq_ignore_ascii_case(right_isrc))
    {
        return true;
    }
    match (left.album.as_deref(), right.album.as_deref()) {
        (Some(left_album), Some(right_album)) => text_close(left_album, right_album, 0.65),
        (Some(_), None) => false,
        _ => true,
    }
}

fn artwork_provider_priority(provider: &str) -> u8 {
    match provider {
        "itunes" | "spotify" => 8,
        "musicbrainz" => 7,
        "soundcloud" => 6,
        "deezer" => 5,
        "discogs" => 4,
        "audd" | "theaudiodb" => 3,
        "lastfm" => 2,
        "youtube" => 1,
        _ => 0,
    }
}

async fn apply_artwork_override(
    pool: &sqlx::SqlitePool,
    path: &Path,
    candidates: &mut [providers::Candidate],
) -> Result<()> {
    let override_row: Option<(String, String, String)> =
        sqlx::query_as("SELECT title,artist,cover_url FROM artwork_overrides WHERE path=?")
            .bind(path.to_string_lossy().as_ref())
            .fetch_optional(pool)
            .await?;
    let Some((title, artist, cover_url)) = override_row else {
        return Ok(());
    };
    let reference = providers::Candidate {
        title,
        artist,
        ..Default::default()
    };
    for candidate in candidates
        .iter_mut()
        .filter(|candidate| candidate_agrees(candidate, &reference))
    {
        candidate.cover_url = Some(cover_url.clone());
        let mut breakdown = candidate
            .score_breakdown
            .as_deref()
            .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        let artwork = breakdown["artwork_candidates"]
            .as_array_mut()
            .map(|items| items as &mut Vec<serde_json::Value>);
        if let Some(items) = artwork {
            items.insert(
                0,
                serde_json::json!({"provider":"User verified","url":cover_url.clone()}),
            );
        } else {
            breakdown["artwork_candidates"] = serde_json::json!([
                {"provider":"User verified","url":cover_url.clone()}
            ]);
        }
        breakdown["artwork_override"] = serde_json::json!(true);
        candidate.score_breakdown = Some(breakdown.to_string());
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
        ]
        .join("|");
        seen.insert(key)
    });
}

fn normalize_match_key(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn candidate_has_fingerprint(candidate: &providers::Candidate) -> bool {
    candidate
        .score_breakdown
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .is_some_and(|value| {
            value["acoustid"].as_f64().is_some_and(|score| score > 0.0)
                || value["audio_recognition"].as_bool() == Some(true)
        })
}

fn candidate_source_count(candidate: &providers::Candidate) -> usize {
    candidate_source_list(candidate).len()
}

fn unique_exact_catalog_match(
    best: &providers::Candidate,
    candidates: &[providers::Candidate],
    current: &audio::AudioInfo,
) -> bool {
    if !matches!(
        best.provider.as_str(),
        "itunes" | "musicbrainz" | "deezer" | "spotify"
    ) || best.duration_delta.is_none_or(|delta| delta > 5.0)
    {
        return false;
    }
    let Some(title) = current.title.as_deref() else {
        return false;
    };
    let Some(artist) = current.artist.as_deref() else {
        return false;
    };
    if title_similarity(title, &best.title) < 0.94 || artist_similarity(artist, &best.artist) < 0.90
    {
        return false;
    }
    let current_album = current
        .album
        .as_deref()
        .filter(|album| !album.trim().is_empty() && !album.trim().starts_with('@'));
    let best_album_similarity = current_album
        .zip(best.album.as_deref())
        .map(|(album, candidate_album)| text_similarity(album, candidate_album));
    if best_album_similarity.is_some_and(|similarity| similarity < 0.65) {
        return false;
    }
    let required_album_similarity = if best_album_similarity.is_some_and(|value| value >= 0.90) {
        0.90
    } else {
        0.65
    };
    candidates
        .iter()
        .filter(|candidate| {
            title_similarity(title, &candidate.title) >= 0.94
                && artist_similarity(artist, &candidate.artist) >= 0.90
                && candidate.duration_delta.is_some_and(|delta| delta <= 5.0)
                && current_album.is_none_or(|album| {
                    candidate.album.as_deref().is_some_and(|candidate_album| {
                        text_similarity(album, candidate_album) >= required_album_similarity
                    })
                })
        })
        .map(|candidate| normalize_match_key(candidate.album.as_deref().unwrap_or_default()))
        .collect::<HashSet<_>>()
        .len()
        == 1
}

fn title_similarity(left: &str, right: &str) -> f64 {
    let left_key = normalize_match_key(left);
    let right_key = normalize_match_key(right);
    let left_words = normalized_words(left);
    let right_words = normalized_words(right);
    let (shorter_words, longer_words) = if left_words.len() <= right_words.len() {
        (&left_words, &right_words)
    } else {
        (&right_words, &left_words)
    };
    if (left_key.len().min(right_key.len()) >= 4
        && (left_key.starts_with(&right_key) || right_key.starts_with(&left_key)))
        || (shorter_words.len() >= 3
            && shorter_words.iter().all(|word| longer_words.contains(word)))
    {
        0.96
    } else {
        text_similarity(left, right)
    }
}

fn artist_similarity(left: &str, right: &str) -> f64 {
    let direct = text_similarity(left, right);
    let left_key = normalize_match_key(left);
    let right_key = normalize_match_key(right);
    if left_key.len().min(right_key.len()) >= 5
        && (left_key.starts_with(&right_key) || right_key.starts_with(&left_key))
    {
        direct.max(0.92)
    } else {
        direct
    }
}

fn normalized_words(value: &str) -> Vec<String> {
    value
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
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
    if left
        .isrc
        .as_deref()
        .zip(right.isrc.as_deref())
        .is_some_and(|(left_isrc, right_isrc)| left_isrc.eq_ignore_ascii_case(right_isrc))
    {
        return true;
    }
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
        "deezer" => "Deezer",
        "audd" => "AudD",
        "itunes" => "Apple Music",
        "lastfm" => "Last.fm",
        "soundcloud" => "SoundCloud",
        "spotify" => "Spotify",
        "theaudiodb" => "TheAudioDB",
        "wikidata" => "Wikidata",
        "youtube" => "YouTube",
        _ => "MusicBrainz",
    }
}

fn provider_reason(provider: &str) -> &str {
    match provider {
        "discogs" => "Release, label, catalog, and physical media metadata",
        "deezer" => "International track, album, ISRC, duration, and cover metadata",
        "audd" => "Audio recognition with ISRC and linked catalog metadata",
        "lastfm" => "Track popularity, tags, and MusicBrainz ID evidence",
        "soundcloud" => "Creator-uploaded title, artist, genre, date, duration, and artwork",
        "spotify" => "ISRC, release, track position, duration, and cover metadata",
        "theaudiodb" => "Track, album, genre, and image enrichment",
        "wikidata" => "Structured identifier and external-link evidence",
        "youtube" => "Exact source-video title, channel, date, and duration evidence",
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

async fn persist_corrupt(
    pool: &sqlx::SqlitePool,
    path: &Path,
    info: &audio::AudioInfo,
    diagnostic: &str,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let id = upsert_track_outcome(
        &mut tx,
        path,
        Some(info),
        "corrupt",
        "failed",
        Some("Audio file is damaged and cannot be written safely"),
        Some(diagnostic),
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
    fn artwork_fallbacks_keep_matching_catalog_covers_only() {
        let mut candidates = vec![
            providers::Candidate {
                provider: "musicbrainz".into(),
                title: "Song".into(),
                artist: "Artist".into(),
                album: Some("Album".into()),
                cover_url: Some("https://example.test/musicbrainz.jpg".into()),
                score: 94.0,
                ..Default::default()
            },
            providers::Candidate {
                provider: "itunes".into(),
                title: "Song".into(),
                artist: "Artist".into(),
                album: Some("Album".into()),
                cover_url: Some("https://example.test/correct.jpg".into()),
                score: 91.0,
                ..Default::default()
            },
            providers::Candidate {
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
        assert!(crate::domain::matcher::auto_selectable(
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

    #[test]
    fn equivalent_catalog_releases_are_one_exact_match() {
        let info = audio::AudioInfo {
            title: Some("Колыбельная".into()),
            artist: Some("Jah Khalib".into()),
            album: Some("@radiop0l".into()),
            ..Default::default()
        };
        let candidates = vec![
            providers::Candidate {
                provider: "itunes".into(),
                title: "Колыбельная".into(),
                artist: "Jah Khalib".into(),
                album: Some("E.G.O.".into()),
                duration_delta: Some(0.14),
                ..Default::default()
            },
            providers::Candidate {
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
        let candidates = vec![providers::Candidate {
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
        let mut candidate = providers::Candidate {
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
}
