use super::*;
use crate::app::ActivityLogEntry;
use chrono::Utc;

pub async fn start_apply(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    if s.workflow_running().await {
        return Err(ApiError::conflict("workflow is already running"));
    }
    let tracks: Vec<Track> = sqlx::query_as(&format!(
        "SELECT {} FROM tracks WHERE selected_candidate_id IS NOT NULL AND is_missing=0",
        queries::TRACK_FIELDS
    ))
    .fetch_all(&s.pool)
    .await?;
    let cfg = s.config.read().await.clone();
    let selected = queries::selected_for_tracks(&s.pool, tracks).await?;
    if selected.is_empty() {
        return Err(ApiError::validation(
            "No identified tracks are ready to write",
        ));
    }
    let mut items = Vec::new();
    for (track, candidate) in selected {
        let dest = destination(&cfg, &track, &candidate)?;
        items.push(PreviewItem {
            track_id: track.id,
            filename: track.filename.clone(),
            current_path: track.path.clone(),
            destination_path: dest,
        });
    }
    let count = items.len();
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
    Ok(Json(serde_json::json!({"started": true, "count": count})))
}

pub async fn apply(s: Arc<AppState>, items: Vec<PreviewItem>) -> Result<()> {
    let total = items.len() as i64;
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
                })),
        )
        .await;
        let (_, candidate) = queries::selected(&s.pool, item.track_id).await?;
        let artwork = if let Some(url) = &candidate.cover_url {
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
                            ActivityLogEntry::new("warn", "artwork", "Cover art download failed")
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
                            ActivityLogEntry::new("warn", "artwork", "Cover art download failed")
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
        };
        let src = PathBuf::from(&item.current_path);
        let dest = PathBuf::from(&item.destination_path);
        let target = {
            if let Some(parent) = dest.parent()
                && let Err(error) = tokio::fs::create_dir_all(parent).await
            {
                s.log_entry(
                    ActivityLogEntry::new("error", "apply", "Failed to create output directory")
                        .file(item.filename.clone())
                        .error(&error)
                        .context(serde_json::json!({"directory": parent.display().to_string()})),
                )
                .await;
                return Err(error.into());
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
            dest
        };
        let write_target = target.clone();
        let write_limiter = s.tag_writes.read().await.clone();
        let write_permit = write_limiter.acquire_owned().await?;
        let result = tokio::task::spawn_blocking({
            move || {
                let _permit = write_permit;
                tag_writer::write(&write_target, &candidate, artwork)?;
                Ok::<_, anyhow::Error>(())
            }
        })
        .await?;
        let (status, error) = match result {
            Ok(_) => ("applied", None),
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
        if status == "failed" {
            s.increment_failed().await;
            let _ = tokio::fs::remove_file(&target).await;
        }
        let final_path = &target;
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
