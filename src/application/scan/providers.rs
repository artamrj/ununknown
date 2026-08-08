use super::*;


pub(crate) async fn identify(
    state: &Arc<AppState>,
    cfg: &crate::config::Config,
    limits: &Arc<PipelineLimits>,
    path: &Path,
    fingerprint: FingerprintEvidence<'_>,
    current: &audio::AudioInfo,
    filename: &str,
) -> Result<Vec<infra_providers::Candidate>> {
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
    out.extend(query_musicbrainz_text(state, limits, current, filename).await);
    match query_itunes(state, cfg, current, filename).await {
        Ok(candidates) => out.extend(candidates),
        Err(error) => handle_provider_error(state, limits, filename, "itunes", error).await,
    }
    out.extend(query_deezer(state, limits, current, filename).await);
    out.extend(query_radiojavan(state, limits, current, filename).await);
    out.extend(query_audiomack(state, limits, current, filename).await);
    out.extend(query_navahang(state, limits, current, filename).await);
    if needs_genius_enrichment(&out) {
        out.extend(query_genius(state, limits, current, filename).await);
    }
    let has_strong_free_candidate = out.iter().any(|candidate| candidate.score >= 90.0);
    let mut recognized_info = None;
    let mut songrec_matched = false;
    if !acoustid_matched && !has_strong_free_candidate && infra_providers::songrec::available() {
        let songrec_candidates =
            query_songrec(state, limits, path, fingerprint.value, filename).await;
        if let Some(recognized) = songrec_candidates.first() {
            let info = audio_info_from_recognition(recognized, current);
            out.extend(query_recognized_catalogs(state, cfg, limits, &info, filename).await);
            recognized_info = Some(info);
            songrec_matched = true;
        }
        out.extend(songrec_candidates);
    }
    if !acoustid_matched
        && !has_strong_free_candidate
        && !songrec_matched
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
            let info = audio_info_from_recognition(recognized, current);
            out.extend(query_recognized_catalogs(state, cfg, limits, &info, filename).await);
            recognized_info = Some(info);
        }
        out.extend(audd_candidates);
    }
    let catalog_info = recognized_info.as_ref().unwrap_or(current);
    if !cfg.spotify_client_id.trim().is_empty() && !cfg.spotify_client_secret.trim().is_empty() {
        let isrcs = out
            .iter()
            .filter_map(|candidate| candidate.isrc.clone())
            .collect::<Vec<_>>();
        out.extend(query_spotify(state, limits, cfg, catalog_info, filename, &isrcs).await);
    }
    if !cfg.soundcloud_client_id.trim().is_empty()
        && !cfg.soundcloud_client_secret.trim().is_empty()
    {
        out.extend(query_soundcloud(state, limits, catalog_info, filename).await);
    }
    out.extend(query_discogs(state, limits, catalog_info, filename).await);
    out.extend(query_lastfm(state, limits, catalog_info, filename).await);
    out.extend(query_theaudiodb(state, limits, catalog_info, filename).await);
    out.extend(query_wikidata(state, limits, catalog_info, filename).await);
    for candidate in &mut out {
        normalize_candidate_credits(candidate);
    }
    crate::application::canonical_names::canonicalize_candidates(&state.pool, &mut out).await?;
    apply_source_agreement(&mut out)?;
    enrich_artwork_fallbacks(&mut out)?;
    apply_artwork_override(&state.pool, path, &mut out).await?;
    let artist_genres = if crate::domain::genre::needs_artist_lookup(&out, catalog_info) {
        if let Some(artist) = catalog_info.artist.as_deref() {
            match infra_providers::wikidata::artist_genres(&state.pool, &state.client, artist).await {
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
    crate::domain::genre::enrich(&mut out, catalog_info, &artist_genres)?;
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

pub(crate) fn sort_candidates_for_decision(candidates: &mut [infra_providers::Candidate]) {
    candidates.sort_by(|a, b| {
        candidate_trust_tier(b)
            .cmp(&candidate_trust_tier(a))
            .then_with(|| b.score.total_cmp(&a.score))
    });
}

pub(crate) fn candidate_trust_tier(candidate: &infra_providers::Candidate) -> u8 {
    if candidate_has_fingerprint(candidate) {
        3
    } else if matches!(
        candidate.provider.as_str(),
        "musicbrainz"
            | "itunes"
            | "deezer"
            | "spotify"
            | "radiojavan"
            | "audiomack"
            | "navahang"
            | "genius"
    ) {
        2
    } else if candidate_source_count(candidate) >= 2 {
        1
    } else {
        0
    }
}

pub(crate) fn audio_info_from_recognition(
    recognized: &infra_providers::Candidate,
    current: &audio::AudioInfo,
) -> audio::AudioInfo {
    audio::AudioInfo {
        title: Some(recognized.title.clone()),
        artist: Some(recognized.artist.clone()),
        album: recognized.album.clone(),
        album_artist: recognized.album_artist.clone(),
        duration: current.duration,
        format: current.format.clone(),
        ..Default::default()
    }
}

pub(crate) async fn query_recognized_catalogs(
    state: &Arc<AppState>,
    cfg: &crate::config::Config,
    limits: &Arc<PipelineLimits>,
    recognized: &audio::AudioInfo,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    let mut out = Vec::new();
    out.extend(query_musicbrainz_text(state, limits, recognized, filename).await);
    match query_itunes(state, cfg, recognized, filename).await {
        Ok(candidates) => out.extend(candidates),
        Err(error) => handle_provider_error(state, limits, filename, "itunes", error).await,
    }
    out.extend(query_deezer(state, limits, recognized, filename).await);
    out.extend(query_radiojavan(state, limits, recognized, filename).await);
    out.extend(query_audiomack(state, limits, recognized, filename).await);
    out.extend(query_navahang(state, limits, recognized, filename).await);
    if needs_genius_enrichment(&out) {
        out.extend(query_genius(state, limits, recognized, filename).await);
    }
    out
}

/// Shared scaffolding for metadata-catalog lookups: disabled-provider gate,
/// optional config-key gate, title gate, timing, error handling, and the
/// common text-scoring pass. Provider-specific steps (AcoustID, SongRec, AudD,
/// YouTube, the ISRC-boosted Spotify search, and the alias-enriching iTunes
/// search) stay explicit in `identify`.
/// Per-provider knobs for the shared catalog runner.
struct CatalogSpec {
    /// Provider id used for the disabled gate, skip/error logs, and timing.
    provider: &'static str,
    /// `score_breakdown` source tag passed to `score_text_candidate`.
    source: &'static str,
    /// Gate the lookup on a non-empty current title.
    requires_title: bool,
    /// Config gate that returns a skip reason when a required key is missing.
    key_check: fn(&crate::config::Config) -> Option<&'static str>,
}

async fn query_catalog<F, P>(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    current: &audio::AudioInfo,
    filename: &str,
    spec: CatalogSpec,
    search: F,
) -> Vec<infra_providers::Candidate>
where
    F: FnOnce(Arc<AppState>, audio::AudioInfo) -> P,
    P: std::future::Future<Output = Result<Vec<infra_providers::Candidate>>>,
{
    if provider_disabled(limits, spec.provider).await {
        return Vec::new();
    }
    let key_reason = {
        let cfg = state.config.read().await;
        (spec.key_check)(&cfg)
    };
    if let Some(reason) = key_reason {
        log_provider_skip(state, filename, spec.provider, reason).await;
        return Vec::new();
    }
    if spec.requires_title
        && current
            .title
            .as_deref()
            .is_none_or(|title| title.trim().is_empty())
    {
        return Vec::new();
    }
    let started = Instant::now();
    match search(state.clone(), current.clone()).await {
        Ok(mut candidates) => {
            for candidate in &mut candidates {
                let _ = score_text_candidate(candidate, current, spec.source);
            }
            log_provider_count(state, filename, spec.provider, candidates.len(), started).await;
            candidates
        }
        Err(error) => {
            handle_provider_error(state, limits, filename, spec.provider, error).await;
            Vec::new()
        }
    }
}

pub(crate) async fn query_soundcloud(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    query_catalog(
        state,
        limits,
        current,
        filename,
        CatalogSpec {
            provider: "soundcloud",
            source: "soundcloud_track_search",
            requires_title: true,
            key_check: |_| None,
        },
        |state, current| async move {
            let cfg = state.config.read().await;
            let title = current.title.as_deref().unwrap_or_default();
            infra_providers::soundcloud::search(
                &state.client,
                &state.soundcloud_auth,
                &cfg.soundcloud_client_id,
                &cfg.soundcloud_client_secret,
                title,
                current.artist.as_deref(),
            )
            .await
        },
    )
    .await
}

pub(crate) async fn query_songrec(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    path: &Path,
    fingerprint: &str,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    if provider_disabled(limits, "songrec").await {
        return Vec::new();
    }
    let started = Instant::now();
    let result = match limits.songrec.acquire().await {
        Ok(_permit) => infra_providers::songrec::recognize(&state.pool, path, fingerprint).await,
        Err(error) => Err(error.into()),
    };
    match result {
        Ok(mut candidates) => {
            for candidate in &mut candidates {
                normalize_candidate_credits(candidate);
            }
            log_provider_count(state, filename, "songrec", candidates.len(), started).await;
            candidates
        }
        Err(error) => {
            handle_provider_error(state, limits, filename, "songrec", error).await;
            Vec::new()
        }
    }
}

pub(crate) async fn query_audd(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    token: &str,
    path: &Path,
    fingerprint: &str,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    if provider_disabled(limits, "audd").await {
        return Vec::new();
    }
    let started = Instant::now();
    match infra_providers::audd::recognize(
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

pub(crate) async fn query_youtube(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    api_key: &str,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    if provider_disabled(limits, "youtube").await {
        return Vec::new();
    }
    let started = Instant::now();
    match infra_providers::youtube::lookup_filename_id(&state.pool, &state.client, api_key, filename)
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

pub(crate) async fn query_spotify(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    cfg: &crate::config::Config,
    current: &audio::AudioInfo,
    filename: &str,
    isrcs: &[String],
) -> Vec<infra_providers::Candidate> {
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
    match infra_providers::spotify::search(
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



pub(crate) async fn query_itunes(
    state: &Arc<AppState>,
    cfg: &crate::config::Config,
    current: &audio::AudioInfo,
    filename: &str,
) -> Result<Vec<infra_providers::Candidate>> {
    let Some(title) = current
        .title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(Vec::new());
    };
    let started = Instant::now();
    let mut candidates = infra_providers::itunes::search(
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
        let aliases = infra_providers::musicbrainz::artist_aliases(
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
                infra_providers::itunes::search(&state.pool, &state.client, "", Some(&alias), None).await
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

pub(crate) async fn query_deezer(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    query_catalog(
        state,
        limits,
        current,
        filename,
        CatalogSpec {
            provider: "deezer",
            source: "deezer_catalog_search",
            requires_title: true,
            key_check: |_| None,
        },
        |state, current| async move {
            let title = current.title.as_deref().unwrap_or_default();
            infra_providers::deezer::search(
                &state.pool,
                &state.client,
                title,
                current.artist.as_deref(),
            )
            .await
        },
    )
    .await
}


pub(crate) async fn query_radiojavan(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    query_catalog(
        state,
        limits,
        current,
        filename,
        CatalogSpec {
            provider: "radiojavan",
            source: "radiojavan_catalog_search",
            requires_title: true,
            key_check: |_| None,
        },
        |state, current| async move {
            let title = current.title.as_deref().unwrap_or_default();
            infra_providers::radiojavan::search(
                &state.pool,
                &state.client,
                title,
                current.artist.as_deref(),
            )
            .await
        },
    )
    .await
}


pub(crate) async fn query_audiomack(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    query_catalog(
        state,
        limits,
        current,
        filename,
        CatalogSpec {
            provider: "audiomack",
            source: "audiomack_catalog_search",
            requires_title: true,
            key_check: |_| None,
        },
        |state, current| async move {
            let title = current.title.as_deref().unwrap_or_default();
            infra_providers::audiomack::search(
                &state.pool,
                &state.client,
                title,
                current.artist.as_deref(),
            )
            .await
        },
    )
    .await
}


pub(crate) async fn query_navahang(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    query_catalog(
        state,
        limits,
        current,
        filename,
        CatalogSpec {
            provider: "navahang",
            source: "navahang_catalog_search",
            requires_title: true,
            key_check: |_| None,
        },
        |state, current| async move {
            let title = current.title.as_deref().unwrap_or_default();
            infra_providers::navahang::search(
                &state.pool,
                &state.client,
                title,
                current.artist.as_deref(),
            )
            .await
        },
    )
    .await
}


pub(crate) async fn query_genius(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    query_catalog(
        state,
        limits,
        current,
        filename,
        CatalogSpec {
            provider: "genius",
            source: "genius_catalog_search",
            requires_title: true,
            key_check: |_| None,
        },
        |state, current| async move {
            let title = current.title.as_deref().unwrap_or_default();
            infra_providers::genius::search(
                &state.pool,
                &state.client,
                title,
                current.artist.as_deref(),
            )
            .await
        },
    )
    .await
}


pub(crate) fn needs_genius_enrichment(candidates: &[infra_providers::Candidate]) -> bool {
    !candidates.iter().any(|candidate| {
        candidate.score >= 90.0
            && candidate
                .album
                .as_deref()
                .is_some_and(|album| !album.trim().is_empty())
            && candidate
                .cover_url
                .as_deref()
                .is_some_and(|cover| !cover.trim().is_empty())
    })
}

pub(crate) async fn query_musicbrainz_acoustid(
    state: &Arc<AppState>,
    cfg: &crate::config::Config,
    limits: &Arc<PipelineLimits>,
    fingerprint: &str,
    duration: f64,
    current: &audio::AudioInfo,
    filename: &str,
) -> Result<Vec<infra_providers::Candidate>> {
    let mut out = Vec::new();
    if cfg.acoustid_key.is_empty() || fingerprint.is_empty() {
        log_provider_skip(state, filename, "acoustid", "AcoustID API key is missing").await;
        return Ok(out);
    }
    let started = Instant::now();
    let hits = {
        let _permit = limits.acoustid.acquire().await?;
        infra_providers::acoustid::lookup(
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
        let mut candidate = infra_providers::musicbrainz::recording(
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
        let outcome = crate::domain::matcher::score_candidate(
            current,
            crate::domain::matcher::CandidateInput {
                title: &candidate.title,
                artist: &candidate.artist,
                album: candidate.album.as_deref(),
                candidate_duration,
            },
            crate::domain::matcher::ScoreMode::Acoustid {
                acoustid_score: hit.score,
                is_compilation: candidate.is_compilation,
            },
            &candidate.provider,
        )?;
        candidate.score = outcome.score;
        candidate.duration_delta = outcome.duration_delta;
        candidate.score_breakdown = Some(outcome.breakdown_json);
        out.push(candidate);
    }
    Ok(out)
}

pub(crate) async fn query_musicbrainz_text(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    query_catalog(
        state,
        limits,
        current,
        filename,
        CatalogSpec {
            provider: "musicbrainz",
            source: "musicbrainz_text_search",
            requires_title: true,
            key_check: |_| None,
        },
        |state, current| async move {
            let cfg = state.config.read().await;
            let title = current.title.as_deref().unwrap_or_default();
            infra_providers::musicbrainz::search(
                &state.pool,
                &state.client,
                &cfg.musicbrainz_user_agent,
                title,
                current.artist.as_deref(),
            )
            .await
        },
    )
    .await
}


pub(crate) async fn query_discogs(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    query_catalog(
        state,
        limits,
        current,
        filename,
        CatalogSpec {
            provider: "discogs",
            source: "discogs_release_search",
            requires_title: false,
            key_check: |cfg| {
                cfg.discogs_token
                    .trim()
                    .is_empty()
                    .then_some("Discogs token/API key is missing")
            },
        },
        |state, current| async move {
            let token = {
                let cfg = state.config.read().await;
                cfg.discogs_token.trim().to_owned()
            };
            infra_providers::discogs::search(&state.pool, &state.client, Some(&token), &current).await
        },
    )
    .await
}


pub(crate) async fn query_lastfm(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    query_catalog(
        state,
        limits,
        current,
        filename,
        CatalogSpec {
            provider: "lastfm",
            source: "lastfm_track_search",
            requires_title: false,
            key_check: |cfg| {
                cfg.lastfm_key
                    .trim()
                    .is_empty()
                    .then_some("Last.fm API key is missing")
            },
        },
        |state, current| async move {
            let api_key = {
                let cfg = state.config.read().await;
                cfg.lastfm_key.trim().to_owned()
            };
            infra_providers::lastfm::search(&state.pool, &state.client, &api_key, &current).await
        },
    )
    .await
}


pub(crate) async fn query_theaudiodb(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    query_catalog(
        state,
        limits,
        current,
        filename,
        CatalogSpec {
            provider: "theaudiodb",
            source: "theaudiodb_track_search",
            requires_title: false,
            key_check: |cfg| {
                cfg.theaudiodb_key
                    .trim()
                    .is_empty()
                    .then_some("TheAudioDB API key is missing")
            },
        },
        |state, current| async move {
            let api_key = {
                let cfg = state.config.read().await;
                cfg.theaudiodb_key.trim().to_owned()
            };
            infra_providers::theaudiodb::search(&state.pool, &state.client, &api_key, &current).await
        },
    )
    .await
}


pub(crate) async fn query_wikidata(
    state: &Arc<AppState>,
    limits: &Arc<PipelineLimits>,
    current: &audio::AudioInfo,
    filename: &str,
) -> Vec<infra_providers::Candidate> {
    query_catalog(
        state,
        limits,
        current,
        filename,
        CatalogSpec {
            provider: "wikidata",
            source: "wikidata_sparql",
            requires_title: false,
            key_check: |_| None,
        },
        |state, current| async move {
            infra_providers::wikidata::search(&state.pool, &state.client, &current).await
        },
    )
    .await
}


pub(crate) fn preserve_album_context_for_catalog_single(
    candidate: &mut infra_providers::Candidate,
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

pub(crate) fn apply_source_agreement(candidates: &mut [infra_providers::Candidate]) -> Result<()> {
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

pub(crate) fn normalize_candidate_credits(candidate: &mut infra_providers::Candidate) {
    let artist = crate::domain::credits::prefer_latin_alias(&candidate.artist);
    let credits = crate::domain::credits::normalize_structured(
        &artist,
        &candidate.title,
        std::mem::take(&mut candidate.artist_credits),
    );
    candidate.artist = credits.artist;
    candidate.title = credits.title;
    candidate.artist_credits = credits.artists;

    if let Some(album_artist) = candidate.album_artist.as_deref() {
        let album_artist = crate::domain::credits::prefer_latin_alias(album_artist);
        let credits = crate::domain::credits::normalize_structured(
            &album_artist,
            "",
            std::mem::take(&mut candidate.album_artist_credits),
        );
        candidate.album_artist = Some(credits.artist);
        candidate.album_artist_credits = credits.artists;
    }
}

pub(crate) fn enrich_artwork_fallbacks(candidates: &mut [infra_providers::Candidate]) -> Result<()> {
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
                        other.clone(),
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
        candidate.artwork_candidates = artwork
            .iter()
            .map(
                |(_, _, provider, url, source)| infra_providers::ArtworkCandidate {
                    provider: provider_display_name(provider).to_owned(),
                    url: url.clone(),
                    release_id: source.release_id.clone(),
                    isrc: source.isrc.clone(),
                    album: source.album.clone(),
                    artist: Some(source.artist.clone()),
                    ..Default::default()
                },
            )
            .collect();
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
                .map(|(_, _, provider, url, _)| {
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

pub(crate) fn artwork_agrees(left: &infra_providers::Candidate, right: &infra_providers::Candidate) -> bool {
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

pub(crate) fn artwork_provider_priority(provider: &str) -> u8 {
    match provider {
        "itunes" | "spotify" => 8,
        "musicbrainz" | "radiojavan" | "audiomack" | "navahang" | "genius" => 7,
        "soundcloud" => 6,
        "deezer" => 5,
        "discogs" => 4,
        "audd" | "theaudiodb" => 3,
        "lastfm" => 2,
        "youtube" => 1,
        _ => 0,
    }
}

pub(crate) async fn apply_artwork_override(
    pool: &sqlx::SqlitePool,
    path: &Path,
    candidates: &mut [infra_providers::Candidate],
) -> Result<()> {
    let override_row: Option<(String, String, String)> =
        sqlx::query_as("SELECT title,artist,cover_url FROM artwork_overrides WHERE path=?")
            .bind(path.to_string_lossy().as_ref())
            .fetch_optional(pool)
            .await?;
    let Some((title, artist, cover_url)) = override_row else {
        return Ok(());
    };
    let reference = infra_providers::Candidate {
        title,
        artist,
        ..Default::default()
    };
    for candidate in candidates
        .iter_mut()
        .filter(|candidate| candidate_agrees(candidate, &reference))
    {
        candidate.cover_url = Some(cover_url.clone());
        candidate.artwork_candidates.insert(
            0,
            infra_providers::ArtworkCandidate {
                provider: "User verified".into(),
                url: cover_url.clone(),
                user_confirmed: true,
                release_id: candidate.release_id.clone(),
                isrc: candidate.isrc.clone(),
                album: candidate.album.clone(),
                artist: Some(candidate.artist.clone()),
            },
        );
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

pub(crate) fn dedupe_candidates(candidates: &mut Vec<infra_providers::Candidate>) {
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

pub(crate) fn candidate_has_fingerprint(candidate: &infra_providers::Candidate) -> bool {
    candidate
        .score_breakdown
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
        .is_some_and(|value| {
            value["acoustid"].as_f64().is_some_and(|score| score > 0.0)
                || value["audio_recognition"].as_bool() == Some(true)
        })
}

pub(crate) fn candidate_source_count(candidate: &infra_providers::Candidate) -> usize {
    candidate_source_list(candidate).len()
}

#[cfg(test)]
pub(crate) fn unique_exact_catalog_match(
    best: &infra_providers::Candidate,
    candidates: &[infra_providers::Candidate],
    current: &audio::AudioInfo,
) -> bool {
    if !matches!(
        best.provider.as_str(),
        "itunes"
            | "musicbrainz"
            | "deezer"
            | "spotify"
            | "radiojavan"
            | "audiomack"
            | "navahang"
            | "genius"
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

pub(crate) fn candidate_agrees(left: &infra_providers::Candidate, right: &infra_providers::Candidate) -> bool {
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

pub(crate) fn candidate_source_list(candidate: &infra_providers::Candidate) -> Vec<String> {
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

pub(crate) fn set_score_sources(candidate: &mut infra_providers::Candidate, sources: Vec<String>) -> Result<()> {
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

pub(crate) async fn log_provider_count(
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

pub(crate) async fn log_provider_skip(state: &Arc<AppState>, filename: &str, provider: &str, reason: &str) {
    state.log("warn", provider, Some(filename), reason).await;
}

pub(crate) async fn log_provider_error(
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

pub(crate) async fn provider_disabled(limits: &Arc<PipelineLimits>, provider: &str) -> bool {
    limits.disabled_providers.lock().await.contains(provider)
}

pub(crate) async fn disable_provider(limits: &Arc<PipelineLimits>, provider: &str) -> bool {
    limits
        .disabled_providers
        .lock()
        .await
        .insert(provider.to_owned())
}

pub(crate) async fn handle_provider_error(
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
