use super::*;
use crate::app::ActivityLogEntry;
use chrono::Utc;

pub async fn stop_apply(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    s.cancel_workflow().await;
    Ok(Json(serde_json::json!({"stopping":true})))
}

pub async fn template_preview(
    State(s): State<Arc<AppState>>,
    Json(body): Json<PreviewRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let base = s.config.read().await.clone();
    let mut cfg = body.settings.unwrap_or(base.clone());
    cfg.metadata_sources.acoustid.api_key = base.metadata_sources.acoustid.api_key.clone();
    cfg.metadata_sources.musicbrainz.user_agent =
        base.metadata_sources.musicbrainz.user_agent.clone();
    cfg.acoustid_api_key = cfg.metadata_sources.acoustid.api_key.clone();
    cfg.musicbrainz_user_agent = cfg.metadata_sources.musicbrainz.user_agent.clone();
    cfg.db_path = base.db_path;
    cfg.validate()
        .map_err(|error| ApiError::validation(error.to_string()))?;
    let sample_track = Track {
        id: TrackId(0),
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
        selected_candidate_id: Some(CandidateId(0)),
        status: "sample".into(),
        error: None,
        is_missing: false,
        stage: TrackStage::Ready,
        stage_message: None,
        retry_count: 0,
        next_retry_at: None,
    };
    let sample_candidate = Candidate {
        id: Some(0),
        provider: "musicbrainz".into(),
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
        release_country: Some("US".into()),
        release_date: Some("2024".into()),
        release_type: Some("Album".into()),
        release_secondary_types: None,
        is_compilation: false,
        duration_delta: Some(0.0),
        score_breakdown: None,
        artist_id: Some("sample-artist".into()),
        album_artist_id: Some("sample-album-artist".into()),
        score: 96.0,
        raw_json: "{}".into(),
    };
    let (track, candidate) = if let Some(id) = body.track_id {
        queries::selected(&s.pool, id).await?
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
    let tracks: Vec<Track> = sqlx::query_as(&format!(
        "SELECT {} FROM tracks WHERE selected_candidate_id IS NOT NULL AND is_missing=0",
        queries::TRACK_FIELDS
    ))
    .fetch_all(&s.pool)
    .await?;
    let cfg = s.config.read().await.clone();
    let selected = queries::selected_for_tracks(&s.pool, tracks).await?;
    let duplicates = duplicate_actions(&selected);
    let duplicate_skipped = duplicates
        .values()
        .filter(|action| action.duplicate_action == DuplicateAction::SkipDuplicate)
        .count();
    let mut items = Vec::new();
    for (track, candidate) in selected {
        let dup = duplicates.get(&track.id).cloned().unwrap_or_default();
        let dest = destination(&cfg, &track, &candidate, None)?;
        let mut warnings = vec![];
        if matches!(track.format.as_deref(), Some("wav" | "aiff" | "aif")) {
            warnings.push("Tag writing will be skipped: conditional/unsafe format".into());
        }
        if candidate.track_number.is_none() {
            if track.current_track_number.is_some() {
                warnings.push(
                    "Candidate has no track number; output path uses the current file track number"
                        .into(),
                );
            } else {
                warnings.push(
                    "Candidate has no track number; output path omits the track prefix".into(),
                );
            }
        }
        if dup.duplicate_action == DuplicateAction::SkipDuplicate {
            warnings.push(dup.duplicate_reason.clone().unwrap_or_else(|| {
                "Duplicate of a stronger matched file; it will not be written".into()
            }));
        }
        let candidate_id = CandidateId(
            candidate
                .id
                .ok_or_else(|| anyhow!("selected candidate is missing a database id"))?,
        );
        items.push(PreviewItem {
            track_id: track.id,
            candidate_id,
            filename: track.filename.clone(),
            current_path: track.path.clone(),
            destination_path: dest,
            action: if cfg.output_mode == OutputMode::Copy {
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
    let token = PreviewToken::new();
    let summary = serde_json::json!({
        "write_count": items.iter().filter(|item| item.duplicate_action != DuplicateAction::SkipDuplicate).count(),
        "duplicate_skipped": duplicate_skipped
    });
    previews::store(
        &s.pool,
        token,
        &items,
        summary.clone(),
        previews::settings_fingerprint(&cfg)?,
        |item| {
            Ok(serde_json::to_string(&item.duplicate_action)?
                .trim_matches('"')
                .to_owned())
        },
        |item| item.track_id.0,
        |item| item.candidate_id.0,
    )
    .await?;
    Ok(Json(serde_json::json!({
        "preview_token":token,
        "items":items,
        "summary":summary
    })))
}
pub async fn start_apply(
    State(s): State<Arc<AppState>>,
    Json(body): Json<ApplyRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    if s.workflow_running().await {
        return Err(ApiError::conflict("workflow is already running"));
    }
    let items = consume_preview(&s.pool, body.preview_token).await?;
    let id = JobId::new();
    s.start_apply_workflow().await;
    let state = s.clone();
    tokio::spawn(async move {
        let result = apply(state.clone(), items).await;
        if state.workflow_cancelled().await {
            state
                .finish_workflow(WorkflowPhase::Idle, "idle", "Apply stopped")
                .await;
        } else if let Err(error) = result {
            state
                .finish_workflow(WorkflowPhase::Failed, "failed", error.to_string())
                .await;
        } else {
            state
                .finish_workflow(WorkflowPhase::Finish, "finish", "Apply complete")
                .await;
        }
    });
    Ok(Json(serde_json::json!({"job_id":id})))
}
fn consume_preview(
    pool: &sqlx::SqlitePool,
    token: PreviewToken,
) -> impl std::future::Future<Output = ApiResult<Vec<PreviewItem>>> + '_ {
    async move {
        previews::consume(pool, token)
            .await
            .map_err(|error| match error {
                previews::PreviewError::AlreadyConsumed => {
                    ApiError::conflict("preview has already been consumed")
                }
                previews::PreviewError::Stale => {
                    ApiError::conflict("preview is stale; run preview again")
                }
                previews::PreviewError::NotUsable => ApiError::conflict("preview is not usable"),
                previews::PreviewError::Missing => {
                    ApiError::not_found("a current successful dry-run preview is required")
                }
            })
    }
}

pub async fn apply(s: Arc<AppState>, items: Vec<PreviewItem>) -> Result<()> {
    let total = items.len() as i64;
    let cfg = s.config.read().await.clone();
    for (i, item) in items.into_iter().enumerate() {
        if s.workflow_cancelled().await {
            break;
        }
        s.log_entry(
            ActivityLogEntry::new("info", "apply", "Applying metadata changes")
                .file(item.filename.clone())
                .context(serde_json::json!({
                    "track_id": item.track_id,
                    "source": item.current_path,
                    "destination": item.destination_path,
                    "action": item.action
                })),
        )
        .await;
        let (_, candidate) = queries::selected(&s.pool, item.track_id).await?;
        let artwork = if cfg.cover_art_enabled {
            if let Some(url) = &candidate.cover_url {
                let limiter = s.artwork_downloads.read().await.clone();
                let _permit = limiter.acquire_owned().await?;
                if let Some(release_id) = candidate.release_id.as_deref() {
                    match crate::infrastructure::providers::cover_art_archive::fetch_cached(
                        &s.pool, &s.client, release_id, url,
                    )
                    .await
                    {
                        Ok(bytes) => {
                            s.log_entry(
                                ActivityLogEntry::new("ok", "artwork", "Downloaded cover art")
                                    .file(item.filename.clone())
                                    .context(serde_json::json!({"release_id": release_id})),
                            )
                            .await;
                            Some(bytes)
                        }
                        Err(error) => {
                            s.log_entry(
                                ActivityLogEntry::new(
                                    "warn",
                                    "artwork",
                                    "Cover art download failed",
                                )
                                .file(item.filename.clone())
                                .error(error.as_ref())
                                .context(serde_json::json!({"release_id": release_id, "url": url})),
                            )
                            .await;
                            None
                        }
                    }
                } else {
                    match crate::infrastructure::providers::cover_art_archive::fetch(&s.client, url)
                        .await
                    {
                        Ok(bytes) => Some(bytes),
                        Err(error) => {
                            s.log_entry(
                                ActivityLogEntry::new(
                                    "warn",
                                    "artwork",
                                    "Cover art download failed",
                                )
                                .file(item.filename.clone())
                                .error(error.as_ref())
                                .context(serde_json::json!({"url": url})),
                            )
                            .await;
                            None
                        }
                    }
                }
            } else {
                None
            }
        } else {
            None
        };
        let src = PathBuf::from(&item.current_path);
        let dest = PathBuf::from(&item.destination_path);
        let target = if cfg.output_mode == OutputMode::Copy {
            if let Some(p) = dest.parent() {
                if let Err(error) = tokio::fs::create_dir_all(p).await {
                    s.log_entry(
                        ActivityLogEntry::new(
                            "error",
                            "apply",
                            "Failed to create output directory",
                        )
                        .file(item.filename.clone())
                        .error(&error)
                        .context(serde_json::json!({"directory": p.display().to_string()})),
                    )
                    .await;
                    return Err(error.into());
                }
            }
            if let Err(error) = tokio::fs::copy(&src, &dest).await {
                s.log_entry(
                    ActivityLogEntry::new("error", "apply", "Failed to copy source file")
                        .file(item.filename.clone())
                        .error(&error)
                        .context(serde_json::json!({
                            "source": src.display().to_string(),
                            "destination": dest.display().to_string()
                        })),
                )
                .await;
                return Err(error.into());
            }
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
        let write_limiter = s.tag_writes.read().await.clone();
        let write_permit = write_limiter.acquire_owned().await?;
        let result = tokio::task::spawn_blocking({
            let cfg = cfg.clone();
            move || {
                let _permit = write_permit;
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
                if cfg.output_mode == OutputMode::InPlace && target != dest {
                    if let Some(parent) = dest.parent() {
                        tokio::fs::create_dir_all(parent).await?;
                    }
                    tokio::fs::rename(&target, &dest).await?;
                    sqlx::query("UPDATE tracks SET path=?,filename=? WHERE id=?")
                        .bind(dest.to_string_lossy())
                        .bind(dest.file_name().and_then(|v| v.to_str()).unwrap_or("audio"))
                        .bind(item.track_id.0)
                        .execute(&s.pool)
                        .await?;
                }
                ("applied", None)
            }
            Err(e) => {
                s.log_entry(
                    ActivityLogEntry::new("error", "apply", "Tag writing failed")
                        .file(item.filename.clone())
                        .error(e.as_ref())
                        .context(serde_json::json!({"target": target.display().to_string()})),
                )
                .await;
                ("failed", Some(format!("{e:#}")))
            }
        };
        if status == "failed" && cfg.output_mode == OutputMode::Copy {
            let _ = tokio::fs::remove_file(&target).await;
        }
        let final_path = if status == "applied" && cfg.output_mode == OutputMode::InPlace {
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
        .bind(item.track_id.0)
        .execute(&s.pool)
        .await?;
        s.set_workflow(
            WorkflowPhase::Apply,
            "apply",
            status,
            i + 1,
            total as usize,
            Some(item.current_path.clone()),
        )
        .await;
        if status == "applied" {
            s.log_entry(
                ActivityLogEntry::new("ok", "apply", "Applied metadata changes")
                    .file(item.filename.clone())
                    .context(serde_json::json!({"output": final_path.display().to_string()})),
            )
            .await;
            sqlx::query("DELETE FROM tracks WHERE id=?")
                .bind(item.track_id.0)
                .execute(&s.pool)
                .await?;
        }
    }
    Ok(())
}
