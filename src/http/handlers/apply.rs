use super::*;
use crate::app::ActivityLogEntry;
use crate::infrastructure::media::replaygain;
use chrono::Utc;
use std::collections::HashSet;

pub async fn start_apply(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    if s.workflow_running().await {
        return Err(ApiError::conflict("workflow is already running"));
    }
    let tracks: Vec<Track> = sqlx::query_as(&format!(
        "SELECT {} FROM tracks WHERE selected_candidate_id IS NOT NULL AND is_missing=0 AND status!='corrupt'",
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
    let mut reserved_destinations = HashSet::new();
    for (track, candidate) in selected {
        let dest = unique_destination(
            PathBuf::from(destination(&cfg, &track, &candidate)?),
            &mut reserved_destinations,
        );
        items.push(PreviewItem {
            track_id: track.id,
            filename: track.filename.clone(),
            current_path: track.path.clone(),
            destination_path: dest.to_string_lossy().into_owned(),
        });
    }
    let count = items.len();
    let delete_source_after_write = cfg.delete_source_after_write;
    s.start_apply_workflow().await;
    let state = s.clone();
    tokio::spawn(async move {
        let result = apply(state.clone(), items, delete_source_after_write).await;
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

fn unique_destination(base: PathBuf, reserved: &mut HashSet<PathBuf>) -> PathBuf {
    if reserved.insert(base.clone()) {
        return base;
    }
    let parent = base.parent().unwrap_or_else(|| std::path::Path::new(""));
    let stem = base
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Corrected track");
    let extension = base.extension().and_then(|value| value.to_str());
    for number in 2.. {
        let filename = match extension {
            Some(extension) => format!("{stem} ({number}).{extension}"),
            None => format!("{stem} ({number})"),
        };
        let candidate = parent.join(filename);
        if reserved.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!()
}

fn temporary_destination(destination: &std::path::Path, track_id: i64) -> PathBuf {
    let parent = destination
        .parent()
        .unwrap_or_else(|| std::path::Path::new(""));
    let stem = destination
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("corrected-track");
    let extension = destination
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or("audio");
    parent.join(format!(".{stem}.ununknown-{track_id}.{extension}"))
}

pub async fn apply(
    s: Arc<AppState>,
    items: Vec<PreviewItem>,
    delete_source_after_write: bool,
) -> Result<()> {
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
        s.set_workflow(
            WorkflowPhase::Apply,
            "replaygain",
            "Analyzing playback loudness",
            i,
            total as usize,
            Some(item.current_path.clone()),
        )
        .await;
        let source_path = std::path::Path::new(&item.current_path);
        let replay_gain = match replaygain::get_or_analyze(&s.pool, source_path).await {
            Ok(value) => {
                s.log_entry(
                    ActivityLogEntry::new("ok", "replaygain", "Measured playback loudness")
                        .file(item.filename.clone())
                        .detail(format!(
                            "Track gain {}; peak {}",
                            value.gain_tag(),
                            value.peak_tag()
                        )),
                )
                .await;
                Some(value)
            }
            Err(error) => {
                // ReplayGain improves compatible players but must never prevent the
                // user's other corrected metadata from being written.
                s.log_entry(
                    ActivityLogEntry::new(
                        "warn",
                        "replaygain",
                        "ReplayGain unavailable; writing other metadata",
                    )
                    .file(item.filename.clone())
                    .error(error.as_ref()),
                )
                .await;
                None
            }
        };
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
        if delete_source_after_write && paths_resolve_to_same_target(&src, &dest).await? {
            anyhow::bail!(
                "refusing to remove source because input and output resolve to the same file: {}",
                src.display()
            );
        }
        let temporary = temporary_destination(&dest, item.track_id.0);
        {
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
            if let Err(error) = tokio::fs::copy(&src, &temporary).await {
                s.log_entry(
                    ActivityLogEntry::new("error", "apply", "Failed to copy source file")
                        .file(item.filename.clone())
                        .error(&error)
                        .context(serde_json::json!({
                            "source": src.display().to_string(),
                            "destination": temporary.display().to_string()
                        })),
                )
                .await;
                return Err(error.into());
            }
        }
        let write_target = temporary.clone();
        let write_limiter = s.tag_writes.read().await.clone();
        let write_permit = write_limiter.acquire_owned().await?;
        let result = tokio::task::spawn_blocking({
            move || {
                let _permit = write_permit;
                tag_writer::write(&write_target, &candidate, artwork, replay_gain)?;
                Ok::<_, anyhow::Error>(())
            }
        })
        .await?;
        let mut result = match result {
            Ok(_) => tokio::fs::rename(&temporary, &dest)
                .await
                .map_err(anyhow::Error::from),
            Err(error) => Err(error),
        };
        let output_was_written = result.is_ok();
        if output_was_written && delete_source_after_write {
            result = remove_source_after_output(&src, &dest).await.map(|_| ());
            if result.is_ok() {
                s.log_entry(
                    ActivityLogEntry::new(
                        "ok",
                        "apply",
                        "Removed original after successful corrected output",
                    )
                    .file(item.filename.clone())
                    .context(serde_json::json!({
                        "source": src.display().to_string(),
                        "output": dest.display().to_string()
                    })),
                )
                .await;
            }
        }
        let (status, error) = match result {
            Ok(_) => ("applied", None),
            Err(e) => {
                s.log_entry(
                    ActivityLogEntry::new(
                        "error",
                        "apply",
                        if output_was_written {
                            "Corrected output was written, but original removal failed"
                        } else {
                            "Tag writing failed"
                        },
                    )
                    .file(item.filename.clone())
                    .error(e.as_ref())
                    .context(serde_json::json!({
                        "temporary": temporary.display().to_string(),
                        "destination": dest.display().to_string()
                    })),
                )
                .await;
                ("failed", Some(format!("{e:#}")))
            }
        };
        if status == "failed" {
            s.increment_failed().await;
            let _ = tokio::fs::remove_file(&temporary).await;
        }
        let final_path = &dest;
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

async fn paths_resolve_to_same_target(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> Result<bool> {
    let source = tokio::fs::canonicalize(source).await?;
    let destination_parent = destination
        .parent()
        .ok_or_else(|| anyhow::anyhow!("output file has no parent directory"))?;
    tokio::fs::create_dir_all(destination_parent).await?;
    let destination = match tokio::fs::canonicalize(destination).await {
        Ok(path) => path,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
            tokio::fs::canonicalize(destination_parent).await?.join(
                destination
                    .file_name()
                    .ok_or_else(|| anyhow::anyhow!("output file has no filename"))?,
            )
        }
        Err(error) => return Err(error.into()),
    };
    Ok(source == destination)
}

async fn remove_source_after_output(
    source: &std::path::Path,
    destination: &std::path::Path,
) -> Result<()> {
    let output = tokio::fs::metadata(destination)
        .await
        .map_err(|error| anyhow::anyhow!("corrected output is unavailable: {error}"))?;
    if !output.is_file() {
        anyhow::bail!("corrected output is not a regular file")
    }
    tokio::fs::remove_file(source)
        .await
        .map_err(|error| anyhow::anyhow!("could not remove original input: {error}"))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn duplicate_destinations_get_numbered_without_overwrite() {
        let mut reserved = HashSet::new();
        let base = PathBuf::from("/output/Artist - Song.mp3");
        assert_eq!(unique_destination(base.clone(), &mut reserved), base);
        assert_eq!(
            unique_destination(base, &mut reserved),
            PathBuf::from("/output/Artist - Song (2).mp3")
        );
    }

    #[test]
    fn temporary_destination_keeps_audio_extension() {
        assert_eq!(
            temporary_destination(std::path::Path::new("/output/Artist - Song.mp3"), 42),
            PathBuf::from("/output/.Artist - Song.ununknown-42.mp3")
        );
    }

    #[tokio::test]
    async fn source_is_removed_only_when_corrected_output_exists() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("input.mp3");
        let output = directory.path().join("output.mp3");
        tokio::fs::write(&source, b"original").await.unwrap();

        assert!(remove_source_after_output(&source, &output).await.is_err());
        assert!(source.exists());

        tokio::fs::write(&output, b"corrected").await.unwrap();
        remove_source_after_output(&source, &output).await.unwrap();
        assert!(!source.exists());
        assert_eq!(tokio::fs::read(&output).await.unwrap(), b"corrected");
    }

    #[tokio::test]
    async fn identical_source_and_destination_are_detected() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("song.mp3");
        tokio::fs::write(&source, b"audio").await.unwrap();
        assert!(
            paths_resolve_to_same_target(&source, &source)
                .await
                .unwrap()
        );
    }
}
