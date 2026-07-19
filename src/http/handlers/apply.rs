use super::*;
use crate::app::ActivityLogEntry;
use crate::infrastructure::fingerprint_cache;
use crate::infrastructure::media::replaygain;
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::collections::HashSet;
use tokio::io::AsyncReadExt;

#[derive(Clone, Debug)]
struct DuplicateSignature {
    isrc: Option<String>,
    title_artist: String,
    duration: Option<f64>,
    fingerprint: Option<String>,
    file_hash: Option<String>,
}

pub async fn start_apply(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    if s.workflow_running().await {
        return Err(ApiError::conflict("workflow is already running"));
    }
    let tracks: Vec<Track> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM tracks WHERE selected_candidate_id IS NOT NULL AND is_missing=0 AND status!='corrupt'",
        queries::TRACK_FIELDS
    )))
    .fetch_all(&s.pool)
    .await?;
    let cfg = s.config.read().await.clone();
    let selected = queries::selected_for_tracks(&s.pool, tracks).await?;
    if selected.is_empty() {
        return Err(ApiError::validation(
            "No identified tracks are ready to write",
        ));
    }
    let selected_count = selected.len();
    let mut items: Vec<PreviewItem> = Vec::new();
    let mut signatures = Vec::new();
    let mut reserved_destinations = HashSet::new();
    for (track, candidate) in selected {
        let signature = duplicate_signature(&s.pool, &track, &candidate).await?;
        if let Some(index) = signatures
            .iter()
            .position(|existing| recordings_are_duplicates(existing, &signature))
        {
            items[index].duplicates.push(DuplicateSource {
                track_id: track.id,
                filename: track.filename,
                current_path: track.path,
            });
            continue;
        }
        let dest = unique_destination(
            PathBuf::from(destination(&cfg, &track, &candidate)?),
            &mut reserved_destinations,
        );
        items.push(PreviewItem {
            track_id: track.id,
            filename: track.filename.clone(),
            current_path: track.path.clone(),
            destination_path: dest.to_string_lossy().into_owned(),
            duplicates: Vec::new(),
        });
        signatures.push(signature);
    }
    let outputs = items.len();
    let duplicates_skipped = selected_count.saturating_sub(outputs);
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
    Ok(Json(serde_json::json!({
        "started": true,
        "count": selected_count,
        "outputs": outputs,
        "duplicates_skipped": duplicates_skipped
    })))
}

async fn duplicate_signature(
    pool: &sqlx::SqlitePool,
    track: &Track,
    candidate: &Candidate,
) -> Result<DuplicateSignature> {
    let path = std::path::Path::new(&track.path);
    let fingerprint = fingerprint_cache::cached(pool, path)
        .await?
        .map(|value| value.fingerprint);
    let file_hash = if fingerprint.is_none() {
        Some(file_sha256(path).await?)
    } else {
        None
    };
    Ok(DuplicateSignature {
        isrc: candidate
            .isrc
            .as_deref()
            .map(normalize_isrc)
            .filter(|value| !value.is_empty()),
        title_artist: format!(
            "{}:{}",
            normalize_identity(&candidate.artist),
            normalize_identity(&candidate.title)
        ),
        duration: track.duration,
        fingerprint,
        file_hash,
    })
}

fn recordings_are_duplicates(left: &DuplicateSignature, right: &DuplicateSignature) -> bool {
    let duration_close = left
        .duration
        .zip(right.duration)
        .is_none_or(|(left, right)| (left - right).abs() <= 3.0);
    if !duration_close {
        return false;
    }
    let same_isrc = left
        .isrc
        .as_deref()
        .zip(right.isrc.as_deref())
        .is_some_and(|(left, right)| left == right);
    if same_isrc {
        return true;
    }
    let same_audio = left
        .fingerprint
        .as_deref()
        .zip(right.fingerprint.as_deref())
        .is_some_and(|(left, right)| left == right)
        || left
            .file_hash
            .as_deref()
            .zip(right.file_hash.as_deref())
            .is_some_and(|(left, right)| left == right);
    same_audio
        && (left.title_artist == right.title_artist
            || left.fingerprint.is_some() && right.fingerprint.is_some())
}

fn normalize_isrc(value: &str) -> String {
    value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_uppercase)
        .collect()
}

fn normalize_identity(value: &str) -> String {
    value
        .chars()
        .flat_map(char::to_lowercase)
        .filter(|character| character.is_alphanumeric())
        .collect()
}

async fn file_sha256(path: &std::path::Path) -> Result<String> {
    let mut file = tokio::fs::File::open(path).await?;
    let mut digest = Sha256::new();
    let mut buffer = vec![0_u8; 128 * 1024];
    loop {
        let count = file.read(&mut buffer).await?;
        if count == 0 {
            break;
        }
        digest.update(&buffer[..count]);
    }
    Ok(hex::encode(digest.finalize()))
}

fn unique_destination(base: PathBuf, reserved: &mut HashSet<PathBuf>) -> PathBuf {
    if !base.exists() && reserved.insert(base.clone()) {
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
        if !candidate.exists() && reserved.insert(candidate.clone()) {
            return candidate;
        }
    }
    unreachable!()
}

fn numbered_destination(base: &std::path::Path, number: usize) -> PathBuf {
    let parent = base.parent().unwrap_or_else(|| std::path::Path::new(""));
    let stem = base
        .file_stem()
        .and_then(|value| value.to_str())
        .unwrap_or("Corrected track");
    let extension = base.extension().and_then(|value| value.to_str());
    let filename = match extension {
        Some(extension) => format!("{stem} ({number}).{extension}"),
        None => format!("{stem} ({number})"),
    };
    parent.join(filename)
}

/// Atomically publishes a completed temporary file without replacing anything
/// already present. A hard link is used because the temporary and final paths
/// are deliberately in the same directory; unlike rename, hard-link creation
/// fails when the destination already exists on every supported platform.
async fn publish_no_clobber(
    temporary: &std::path::Path,
    preferred: &std::path::Path,
) -> Result<PathBuf> {
    tokio::fs::OpenOptions::new()
        .write(true)
        .open(temporary)
        .await?
        .sync_all()
        .await?;

    let mut number = 1;
    loop {
        let destination = if number == 1 {
            preferred.to_owned()
        } else {
            numbered_destination(preferred, number)
        };
        match tokio::fs::hard_link(temporary, &destination).await {
            Ok(()) => {
                if let Err(error) = tokio::fs::remove_file(temporary).await {
                    tracing::warn!(
                        path = %temporary.display(),
                        %error,
                        "published output but could not remove temporary link"
                    );
                }
                return Ok(destination);
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                number += 1;
            }
            Err(error) => return Err(error.into()),
        }
    }
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
        let artwork = resolve_artwork(&s, &item.filename, &candidate).await?;
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
                let expected_artwork = artwork.clone();
                let sanitized =
                    tag_writer::write_resilient(&write_target, &candidate, artwork, replay_gain)?;
                if let Some(expected) = expected_artwork {
                    tag_writer::verify_embedded_artwork(&write_target, &expected)?;
                }
                Ok::<_, anyhow::Error>(sanitized)
            }
        })
        .await?;
        if result.as_ref().is_ok_and(|sanitized| *sanitized) {
            s.log_entry(
                ActivityLogEntry::new(
                    "ok",
                    "tags",
                    "Removed malformed legacy tags with lossless stream-copy and retried",
                )
                .file(item.filename.clone()),
            )
            .await;
        }
        let publication = match result {
            Ok(_) => publish_no_clobber(&temporary, &dest).await,
            Err(error) => Err(error),
        };
        let output_was_written = publication.is_ok();
        let final_path = publication.as_ref().unwrap_or(&dest).clone();
        let mut result = publication.map(|_| ());
        if output_was_written && delete_source_after_write {
            if paths_resolve_to_same_target(&src, &final_path).await? {
                anyhow::bail!(
                    "refusing to remove source because input and output resolve to the same file: {}",
                    src.display()
                );
            }
            result = remove_source_after_output(&src, &final_path)
                .await
                .map(|_| ());
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
                        "output": final_path.display().to_string()
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
                        "destination": final_path.display().to_string()
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
        sqlx::query(
            "UPDATE tracks SET output_path=?,status=?,error=?,last_applied_at=? WHERE id=?",
        )
        .bind(output_was_written.then(|| final_path.to_string_lossy().into_owned()))
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
            for duplicate in &item.duplicates {
                if let Err(error) =
                    finish_duplicate(&s, duplicate, &final_path, delete_source_after_write).await
                {
                    s.increment_failed().await;
                    let detail = format!("{error:#}");
                    let _ = sqlx::query(
                        "UPDATE tracks SET status='failed',stage='failed',error=?,stage_message='Duplicate output was avoided, but source cleanup failed',updated_at=? WHERE id=?",
                    )
                    .bind(&detail)
                    .bind(Utc::now().to_rfc3339())
                    .bind(duplicate.track_id.0)
                    .execute(&s.pool)
                    .await;
                    s.log_entry(
                        ActivityLogEntry::new(
                            "error",
                            "deduplicate",
                            "Duplicate output was avoided, but source cleanup failed",
                        )
                        .file(duplicate.filename.clone())
                        .error_text(detail),
                    )
                    .await;
                }
            }
            sqlx::query("DELETE FROM tracks WHERE id=?")
                .bind(item.track_id.0)
                .execute(&s.pool)
                .await?;
        }
    }
    Ok(())
}

async fn finish_duplicate(
    state: &Arc<AppState>,
    duplicate: &DuplicateSource,
    output: &std::path::Path,
    delete_source_after_write: bool,
) -> Result<()> {
    let source = std::path::Path::new(&duplicate.current_path);
    if delete_source_after_write {
        if paths_resolve_to_same_target(source, output).await? {
            anyhow::bail!(
                "refusing to remove duplicate source because it is the corrected output: {}",
                source.display()
            );
        }
        remove_source_after_output(source, output).await?;
    }
    sqlx::query("DELETE FROM tracks WHERE id=?")
        .bind(duplicate.track_id.0)
        .execute(&state.pool)
        .await?;
    state
        .log_entry(
            ActivityLogEntry::new(
                "ok",
                "deduplicate",
                "Skipped duplicate recording; kept one corrected output",
            )
            .file(duplicate.filename.clone())
            .context(serde_json::json!({
                "source": duplicate.current_path,
                "output": output.display().to_string(),
                "source_removed": delete_source_after_write
            })),
        )
        .await;
    Ok(())
}

pub(super) async fn resolve_artwork(
    state: &Arc<AppState>,
    filename: &str,
    candidate: &crate::infrastructure::providers::Candidate,
) -> Result<Option<Vec<u8>>> {
    let mut urls = Vec::<(String, String)>::new();
    if let Some(url) = candidate.cover_url.as_deref() {
        urls.push((candidate.provider.clone(), url.to_owned()));
    }
    if let Some(value) = candidate
        .score_breakdown
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
    {
        for item in value["artwork_candidates"].as_array().into_iter().flatten() {
            if let Some(url) = item["url"].as_str() {
                urls.push((
                    item["provider"].as_str().unwrap_or("catalog").to_owned(),
                    url.to_owned(),
                ));
            }
        }
    }
    let mut seen = HashSet::new();
    urls.retain(|(_, url)| seen.insert(url.clone()));
    if urls.is_empty() {
        state
            .log(
                "info",
                "artwork",
                Some(filename),
                "No catalog cover was available; preserving a valid embedded cover if present",
            )
            .await;
        return Ok(None);
    }

    let limiter = state.artwork_downloads.read().await.clone();
    let _permit = limiter.acquire_owned().await?;
    for (provider, url) in urls {
        let result = crate::infrastructure::providers::cover_art_archive::fetch_url_cached(
            &state.pool,
            &state.client,
            &url,
        )
        .await;
        match result.and_then(|bytes| {
            crate::infrastructure::media::tag_writer::validate_artwork(&bytes)?;
            Ok(bytes)
        }) {
            Ok(bytes) => {
                state
                    .log_entry(
                        ActivityLogEntry::new("ok", "artwork", "Downloaded valid cover art")
                            .file(filename.to_owned())
                            .context(serde_json::json!({"provider": provider, "url": url})),
                    )
                    .await;
                return Ok(Some(bytes));
            }
            Err(error) => {
                state
                    .log_entry(
                        ActivityLogEntry::new(
                            "warn",
                            "artwork",
                            "Artwork source failed; trying the next matching source",
                        )
                        .file(filename.to_owned())
                        .error(error.as_ref())
                        .context(serde_json::json!({"provider": provider, "url": url})),
                    )
                    .await;
            }
        }
    }
    state
        .log(
            "warn",
            "artwork",
            Some(filename),
            "No catalog artwork could be downloaded; preserving a valid embedded cover if present",
        )
        .await;
    Ok(None)
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
        let directory = tempfile::tempdir().unwrap();
        let mut reserved = HashSet::new();
        let base = directory.path().join("Artist - Song.mp3");
        assert_eq!(unique_destination(base.clone(), &mut reserved), base);
        assert_eq!(
            unique_destination(base.clone(), &mut reserved),
            directory.path().join("Artist - Song (2).mp3")
        );

        std::fs::write(directory.path().join("Artist - Song (3).mp3"), b"existing").unwrap();
        reserved.insert(directory.path().join("Artist - Song (2).mp3"));
        reserved.insert(base.clone());
        assert_eq!(
            unique_destination(base, &mut reserved),
            directory.path().join("Artist - Song (4).mp3")
        );
    }

    #[tokio::test]
    async fn publishing_never_replaces_an_existing_output() {
        let directory = tempfile::tempdir().unwrap();
        let temporary = directory.path().join(".temporary.mp3");
        let preferred = directory.path().join("Artist - Song.mp3");
        tokio::fs::write(&temporary, b"new output").await.unwrap();
        tokio::fs::write(&preferred, b"existing output")
            .await
            .unwrap();

        let published = publish_no_clobber(&temporary, &preferred).await.unwrap();

        assert_eq!(published, directory.path().join("Artist - Song (2).mp3"));
        assert_eq!(
            tokio::fs::read(&preferred).await.unwrap(),
            b"existing output"
        );
        assert_eq!(tokio::fs::read(&published).await.unwrap(), b"new output");
        assert!(!temporary.exists());
    }

    fn signature(
        isrc: Option<&str>,
        identity: &str,
        duration: f64,
        fingerprint: Option<&str>,
        file_hash: Option<&str>,
    ) -> DuplicateSignature {
        DuplicateSignature {
            isrc: isrc.map(normalize_isrc),
            title_artist: identity.into(),
            duration: Some(duration),
            fingerprint: fingerprint.map(str::to_owned),
            file_hash: file_hash.map(str::to_owned),
        }
    }

    #[test]
    fn same_isrc_with_close_duration_is_one_output() {
        let first = signature(
            Some("QZ-DA5-20-82376"),
            "twenty7:eyesonyou",
            148.0,
            None,
            None,
        );
        let second = signature(Some("qzda52082376"), "twenty7:eyesonyou", 149.0, None, None);
        assert!(recordings_are_duplicates(&first, &second));
    }

    #[test]
    fn audio_fingerprint_detects_duplicate_with_different_tags() {
        let first = signature(None, "bad:tags", 238.0, Some("audio-fp"), None);
        let second = signature(None, "hoomaan:darling", 238.4, Some("audio-fp"), None);
        assert!(recordings_are_duplicates(&first, &second));
    }

    #[test]
    fn same_title_does_not_hide_a_different_recording() {
        let first = signature(None, "artist:song", 180.0, Some("first"), None);
        let second = signature(None, "artist:song", 180.0, Some("second"), None);
        assert!(!recordings_are_duplicates(&first, &second));
    }

    #[test]
    fn large_duration_difference_is_not_deduplicated() {
        let first = signature(Some("US1234567890"), "artist:song", 180.0, None, None);
        let second = signature(Some("US1234567890"), "artist:song", 240.0, None, None);
        assert!(!recordings_are_duplicates(&first, &second));
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
    async fn duplicate_source_is_removed_after_the_kept_output_exists() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("duplicates.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let source = directory.path().join("duplicate.mp3");
        let output = directory.path().join("Artist - Song.mp3");
        tokio::fs::write(&source, b"duplicate").await.unwrap();
        tokio::fs::write(&output, b"corrected").await.unwrap();
        let track_id = sqlx::query("INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage) VALUES(?,'duplicate.mp3','selected',0,'now','now','now','ready')")
            .bind(source.to_string_lossy().as_ref())
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
        let state = Arc::new(AppState::new(
            crate::config::Config::default(),
            pool.clone(),
        ));
        let duplicate = DuplicateSource {
            track_id: TrackId(track_id),
            filename: "duplicate.mp3".into(),
            current_path: source.to_string_lossy().into_owned(),
        };

        finish_duplicate(&state, &duplicate, &output, true)
            .await
            .unwrap();

        assert!(!source.exists());
        assert!(output.exists());
        let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM tracks WHERE id=?")
            .bind(track_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, 0);
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
