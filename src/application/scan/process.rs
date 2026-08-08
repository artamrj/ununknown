use super::*;

pub(crate) async fn process_file(
    state: Arc<AppState>,
    limits: Arc<PipelineLimits>,
    persist_tx: mpsc::Sender<PersistJob>,
    job: FileJob,
    total: usize,
) {
    let mut members = job.members.into_iter();
    while let Some(member) = members.next() {
        if state.workflow_cancelled().await {
            return;
        }
        let filename = member
            .path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("audio")
            .to_owned();
        const ATTEMPTS: usize = 2;
        let mut outcome = None;
        for attempt in 1..=ATTEMPTS {
            if state.workflow_cancelled().await {
                return;
            }
            state
                .start_track(
                    total,
                    filename.clone(),
                    format!("Matching {filename} · attempt {attempt}/{ATTEMPTS}"),
                )
                .await;
            match process(&state, &limits, &persist_tx, &member.path).await {
                Ok(result) => {
                    outcome = Some(result);
                    break;
                }
                Err(error) if attempt < ATTEMPTS => {
                    tracing::warn!(path=%member.path.display(), attempt, "track attempt failed: {error:#}");
                    state
                        .log_entry(
                            ActivityLogEntry::new(
                                "warn",
                                "fetch",
                                "Track attempt failed; retrying",
                            )
                            .file(filename.clone())
                            .attempt(attempt as i64)
                            .error_text(format!("{error:#}"))
                            .context(serde_json::json!({
                                "path": member.path.display().to_string(),
                                "max_attempts": ATTEMPTS
                            })),
                        )
                        .await;
                    tokio::time::sleep(Duration::from_millis(400 * attempt as u64)).await;
                }
                Err(error) => {
                    tracing::warn!(path=%member.path.display(), "track failed after retries: {error:#}");
                    state.increment_failed().await;
                    let error_text = format!("{error:#}");
                    if let Err(persist_error) =
                        persist_failed(&state.pool, &member.path, &error_text).await
                    {
                        tracing::warn!(path=%member.path.display(), "failed to persist failed track: {persist_error:#}");
                        state
                            .log_entry(
                                ActivityLogEntry::new(
                                    "error",
                                    "db",
                                    "Failed to persist failed track",
                                )
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
                                    "path": member.path.display().to_string(),
                                    "max_attempts": ATTEMPTS
                                })),
                        )
                        .await;
                }
            }
        }
        if let Err(error) = persist_analysis_identity(&state.pool, &member).await {
            tracing::warn!(path=%member.path.display(), "could not persist input identity: {error:#}");
        }
        match outcome {
            Some(ProcessOutcome::Matched) => {
                state
                    .log(
                        "ok",
                        "fetch",
                        Some(&filename),
                        "Matched and stored for Preview",
                    )
                    .await;
                finish_scan_progress(&state, total).await;
                let duplicates = members.collect::<Vec<_>>();
                persist_duplicate_members(&state, &member, duplicates, total).await;
                return;
            }
            Some(ProcessOutcome::NeedsReview) => {
                state
                    .log(
                        "warn",
                        "fetch",
                        Some(&filename),
                        "No selected match; moving to next file",
                    )
                    .await;
                finish_scan_progress(&state, total).await;
                let duplicates = members.collect::<Vec<_>>();
                persist_duplicate_members(&state, &member, duplicates, total).await;
                return;
            }
            Some(ProcessOutcome::Corrupt) => {
                state
                    .log(
                        "error",
                        "integrity",
                        Some(&filename),
                        "Damaged audio was blocked from metadata writing",
                    )
                    .await;
            }
            None => {}
        }
        finish_scan_progress(&state, total).await;
    }
}

pub(crate) async fn process(
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
    let smart_decision = crate::application::smart_approval::rank(
        crate::application::smart_approval::TrackEvidence {
            filename,
            title: info.title.as_deref(),
            artist: info.artist.as_deref(),
            album: info.album.as_deref(),
        },
        &candidates,
    );
    let Some(smart_decision) = smart_decision else {
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
    };
    let embedded_cover = tokio::task::spawn_blocking({
        let path = path.to_path_buf();
        move || {
            crate::infrastructure::media::tag_writer::read_artwork(&path)
                .ok()
                .flatten()
                .is_some()
        }
    })
    .await
    .unwrap_or(false);
    let mut candidate = candidates[smart_decision.candidate_index].clone();
    crate::application::metadata_completion::complete(
        &mut candidate,
        &candidates,
        info.album.as_deref(),
        embedded_cover,
    );
    crate::application::canonical_names::canonicalize_candidates(
        &state.pool,
        std::slice::from_mut(&mut candidate),
    )
    .await?;
    let limiter = state.artwork_downloads.read().await.clone();
    let _permit = limiter.acquire_owned().await?;
    crate::application::artwork::ensure_usable_cover(
        &state.pool,
        &state.client,
        &mut candidate,
        embedded_cover,
    )
    .await;
    let completion =
        crate::application::metadata_completion::reassess(&mut candidate, embedded_cover);
    state
        .log(
            if completion.core_complete {
                "ok"
            } else {
                "warn"
            },
            "metadata_completion",
            Some(filename),
            &completion.summary(),
        )
        .await;
    if !completion.core_complete {
        state.increment_unmatched().await;
        candidates[smart_decision.candidate_index] = candidate;
        candidates.swap(0, smart_decision.candidate_index);
        let message = format!(
            "A recording match was found, but the metadata worker could not find {}. It remains in review instead of creating an incomplete identification.",
            completion.missing_fields.join(", ")
        );
        persist_review(&state.pool, path, &info, &candidates, &message).await?;
        state.log("warn", "match", Some(filename), &message).await;
        return Ok(ProcessOutcome::NeedsReview);
    }
    let selection_message = format!(
        "Smart auto-selected ({:.0}%): {}; {}",
        smart_decision.confidence,
        smart_decision.explanation,
        completion.summary()
    );
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
            message: selection_message,
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
