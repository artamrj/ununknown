use super::*;

pub async fn stop_apply(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    s.workflow.write().await.cancelled = true;
    Ok(Json(serde_json::json!({"stopping":true})))
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
    store_preview(
        &s.pool,
        token,
        &items,
        summary.clone(),
        settings_fingerprint(&cfg)?,
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
    let items = consume_preview(&s.pool, body.preview_token).await?;
    let id = JobId::new();
    {
        let mut w = s.workflow.write().await;
        w.phase = WorkflowPhase::Apply;
        w.message = "Applying matched metadata".into();
        w.cancelled = false;
    }
    let state = s.clone();
    let job = id;
    tokio::spawn(async move {
        let result = apply(state.clone(), job.clone(), items).await;
        let mut w = state.workflow.write().await;
        w.phase = if result.is_ok() {
            WorkflowPhase::Finish
        } else {
            WorkflowPhase::Failed
        };
        w.message = result
            .err()
            .map(|e| e.to_string())
            .unwrap_or_else(|| "Apply complete".into());
    });
    Ok(Json(serde_json::json!({"job_id":id})))
}
pub async fn apply(s: Arc<AppState>, job: JobId, items: Vec<PreviewItem>) -> Result<()> {
    let total = items.len() as i64;
    let cfg = s.config.read().await.clone();
    for (i, item) in items.into_iter().enumerate() {
        if s.cancelled(&job.to_string()).await {
            break;
        }
        let (_, candidate) = selected(&s.pool, item.track_id).await?;
        let artwork = if cfg.cover_art_enabled {
            if let Some(url) = &candidate.cover_url {
                let limiter = s.artwork_downloads.read().await.clone();
                let _permit = limiter.acquire_owned().await?;
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
        let target = if cfg.output_mode == OutputMode::Copy {
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
            Err(e) => ("failed", Some(e.to_string())),
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
        {
            let mut w = s.workflow.write().await;
            w.current = i + 1;
            w.total = total as usize;
            w.current_file = Some(item.current_path.clone());
        }
        jobs::emit(
            &s,
            "workflow",
            Some("apply"),
            Some(WorkflowPhase::Apply),
            i as i64 + 1,
            total,
            status,
        );
        if status == "applied" {
            sqlx::query("DELETE FROM tracks WHERE id=?")
                .bind(item.track_id.0)
                .execute(&s.pool)
                .await?;
        }
    }
    Ok(())
}
