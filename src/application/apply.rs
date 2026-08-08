use crate::{
    app::{ActivityLogEntry, AppState},
    application::input_dedup::{self, RecordingEvidence},
    config::Config,
    infrastructure::{
        db::tracks::{CandidateRow, TRACK_FIELDS, Track, candidates_by_track, selected_for_tracks},
        fingerprint_cache,
        media::{fingerprint, replaygain, tag_writer},
        providers::Candidate,
    },
    types::{TrackId, WorkflowPhase},
};
use anyhow::{Result, anyhow};
use chrono::Utc;
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::Arc};
use tokio::io::AsyncReadExt;

pub(crate) struct PreparedApply {
    pub(crate) items: Vec<PreviewItem>,
    pub(crate) selected_count: usize,
    pub(crate) outputs: usize,
    pub(crate) duplicates_skipped: usize,
}

#[derive(Clone)]
pub(crate) struct PreviewItem {
    track_id: TrackId,
    filename: String,
    current_path: String,
    destination_path: String,
    candidate: Candidate,
    duplicates: Vec<DuplicateSource>,
}

#[derive(Clone)]
struct DuplicateSource {
    track_id: TrackId,
    filename: String,
    current_path: String,
    source_missing: bool,
}

fn destination(cfg: &Config, track: &Track, candidate: &Candidate) -> Result<String> {
    let source = std::path::Path::new(&track.path);
    let relative = source
        .strip_prefix(&cfg.input_dir)
        .ok()
        .filter(|path| !path.as_os_str().is_empty())
        .or_else(|| source.file_name().map(std::path::Path::new))
        .ok_or_else(|| anyhow!("audio file has no filename"))?;
    let parent = relative
        .parent()
        .unwrap_or_else(|| std::path::Path::new(""));
    // Re-sniff the source at apply time. The database may still contain an old
    // extension-based format from a previous scan (for example, AAC/M4A bytes in
    // a file named `.mp3`). Falling back keeps previews for missing files usable.
    let detected_format = crate::domain::audio::read(source)
        .ok()
        .map(|info| info.format);
    let extension = detected_format
        .as_deref()
        .filter(|value| !value.is_empty())
        .or_else(|| track.format.as_deref().filter(|value| !value.is_empty()))
        .or_else(|| source.extension().and_then(|value| value.to_str()));
    let credits = crate::domain::credits::normalize_featured(&candidate.artist, &candidate.title);
    let artist = safe_filename_part(&credits.artist, "Unknown Artist");
    let title = safe_filename_part(&credits.title, "Unknown Title");
    let mut basename = truncate_utf8(&format!("{artist} - {title}"), 220).to_owned();
    if let Some(extension) = extension {
        basename.push('.');
        basename.push_str(&extension.to_ascii_lowercase());
    }
    Ok(std::path::PathBuf::from(&cfg.output_dir)
        .join(parent)
        .join(basename)
        .to_string_lossy()
        .into_owned())
}

fn safe_filename_part(value: &str, fallback: &str) -> String {
    let cleaned = value
        .chars()
        .map(|ch| {
            if ch.is_control() || matches!(ch, '/' | '\\' | ':' | '*' | '?' | '"' | '<' | '>' | '|')
            {
                ' '
            } else {
                ch
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let cleaned = cleaned.trim_matches([' ', '.']);
    if cleaned.is_empty() {
        fallback.to_owned()
    } else {
        cleaned.to_owned()
    }
}

fn truncate_utf8(value: &str, max_bytes: usize) -> &str {
    if value.len() <= max_bytes {
        return value;
    }
    let mut end = max_bytes;
    while !value.is_char_boundary(end) {
        end -= 1;
    }
    value[..end].trim_end()
}

#[cfg(test)]
mod filename_tests {
    use super::*;

    #[test]
    fn filename_parts_preserve_unicode_and_remove_forbidden_characters() {
        assert_eq!(
            safe_filename_part("  فریدون / فرخزاد:  ", "fallback"),
            "فریدون فرخزاد"
        );
        assert_eq!(safe_filename_part("...", "Unknown Title"), "Unknown Title");
    }

    #[test]
    fn utf8_truncation_does_not_split_character() {
        let value = "آهنگ".repeat(100);
        let truncated = truncate_utf8(&value, 220);
        assert!(truncated.len() <= 220);
        assert!(std::str::from_utf8(truncated.as_bytes()).is_ok());
    }
}

pub(crate) async fn prepare_apply(s: &Arc<AppState>) -> Result<PreparedApply> {
    let ready_tracks: Vec<Track> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM tracks WHERE selected_candidate_id IS NOT NULL AND is_missing=0 AND status!='corrupt' AND stage='ready'",
        TRACK_FIELDS
    )))
    .fetch_all(&s.pool)
    .await?;
    let cfg = s.config.read().await.clone();
    let selected = selected_for_tracks(&s.pool, ready_tracks).await?;
    let skipped_tracks: Vec<Track> = sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {} FROM tracks
         WHERE status='duplicate' AND stage='skipped' AND is_missing=0
           AND content_fingerprint IS NOT NULL
         ORDER BY path",
        TRACK_FIELDS
    )))
    .fetch_all(&s.pool)
    .await?;

    struct PlannedSource {
        track: Track,
        candidate: Option<Candidate>,
        available: bool,
    }
    let mut sources = selected
        .into_iter()
        .map(|(track, candidate)| PlannedSource {
            track,
            candidate: Some(candidate),
            available: false,
        })
        .collect::<Vec<_>>();
    sources.extend(skipped_tracks.into_iter().map(|track| PlannedSource {
        track,
        candidate: None,
        available: false,
    }));

    // Reapply release and identity rules at the last mutable boundary. This
    // also repairs ready-but-unwritten selections created by an older scan.
    let by_track = candidates_by_track(&s.pool).await?;
    for source in &mut sources {
        let track_id = source.track.id;
        let existing_album = source.track.current_album.clone();
        if let Some(candidate) = source.candidate.as_mut() {
            let candidates = by_track
                .get(&track_id)
                .into_iter()
                .flat_map(|rows| rows.iter())
                .map(CandidateRow::value)
                .collect::<Vec<_>>();
            crate::application::metadata_completion::complete(
                candidate,
                &candidates,
                existing_album.as_deref(),
                false,
            );
            crate::application::canonical_names::canonicalize_candidates(
                &s.pool,
                std::slice::from_mut(candidate),
            )
            .await?;
            persist_apply_normalization(&s.pool, candidate).await?;
        }
    }

    let mut evidence = Vec::with_capacity(sources.len());
    for source in &mut sources {
        source.available = tokio::fs::metadata(&source.track.path)
            .await
            .is_ok_and(|metadata| metadata.is_file());
        if !source.available {
            sqlx::query(
                "UPDATE tracks SET is_missing=1,status='failed',stage='failed',
                 stage_message='Source file is missing; restore it and scan again',updated_at=?
                 WHERE id=?",
            )
            .bind(Utc::now().to_rfc3339())
            .bind(source.track.id.0)
            .execute(&s.pool)
            .await?;
        }
        if source.available {
            evidence
                .push(recording_evidence(&s.pool, &source.track, source.candidate.as_ref()).await?);
        } else {
            evidence.push(RecordingEvidence {
                path: source.track.path.clone().into(),
                format: source.track.format.clone().unwrap_or_default(),
                bitrate: source
                    .track
                    .bitrate
                    .and_then(|value| u32::try_from(value).ok()),
                duration: source.track.duration,
                content_key: source.track.content_fingerprint.clone(),
                isrc: source
                    .candidate
                    .as_ref()
                    .and_then(|candidate| candidate.isrc.clone()),
            });
        }
    }

    let mut items: Vec<PreviewItem> = Vec::new();
    let mut selected_count = 0;
    for group in input_dedup::group_recordings(&evidence) {
        let Some((candidate_owner, candidate)) = group.members.iter().find_map(|index| {
            sources[*index]
                .candidate
                .clone()
                .map(|candidate| (*index, candidate))
        }) else {
            continue;
        };
        let Some(representative) = group
            .members
            .iter()
            .copied()
            .find(|index| sources[*index].available)
        else {
            continue;
        };
        if representative != candidate_owner {
            promote_apply_representative(
                &s.pool,
                &sources[candidate_owner].track,
                &sources[representative].track,
                sources[candidate_owner].available,
            )
            .await?;
        }
        selected_count += group
            .members
            .iter()
            .filter(|index| sources[**index].available)
            .count();
        let source = &sources[representative];
        let track = &source.track;
        let dest = PathBuf::from(destination(&cfg, track, &candidate)?);
        let duplicates = group
            .members
            .into_iter()
            .filter(|index| *index != representative)
            .map(|index| DuplicateSource {
                track_id: sources[index].track.id,
                filename: sources[index].track.filename.clone(),
                current_path: sources[index].track.path.clone(),
                source_missing: !sources[index].available,
            })
            .collect();
        items.push(PreviewItem {
            track_id: track.id,
            filename: track.filename.clone(),
            current_path: track.path.clone(),
            destination_path: dest.to_string_lossy().into_owned(),
            candidate,
            duplicates,
        });
    }
    let outputs = items.len();
    let duplicates_skipped = selected_count.saturating_sub(outputs);
    Ok(PreparedApply {
        items,
        selected_count,
        outputs,
        duplicates_skipped,
    })
}

async fn persist_apply_normalization(pool: &sqlx::SqlitePool, candidate: &Candidate) -> Result<()> {
    let Some(id) = candidate.id else {
        return Ok(());
    };
    sqlx::query(
        "UPDATE candidates SET title=?,artist=?,artist_credits_json=?,album=?,album_artist=?,
         album_artist_credits_json=?,release_type=? WHERE id=?",
    )
    .bind(&candidate.title)
    .bind(&candidate.artist)
    .bind(serde_json::to_string(&candidate.artist_credits)?)
    .bind(&candidate.album)
    .bind(&candidate.album_artist)
    .bind(serde_json::to_string(&candidate.album_artist_credits)?)
    .bind(&candidate.release_type)
    .bind(id)
    .execute(pool)
    .await?;
    Ok(())
}

async fn promote_apply_representative(
    pool: &sqlx::SqlitePool,
    candidate_owner: &Track,
    representative: &Track,
    owner_available: bool,
) -> Result<()> {
    let candidate_id = candidate_owner
        .selected_candidate_id
        .ok_or_else(|| anyhow!("duplicate representative has no selected candidate"))?;
    let now = Utc::now().to_rfc3339();
    let mut transaction = pool.begin().await?;
    sqlx::query("DELETE FROM candidates WHERE track_id=?")
        .bind(representative.id.0)
        .execute(&mut *transaction)
        .await?;
    sqlx::query("UPDATE candidates SET track_id=? WHERE track_id=?")
        .bind(representative.id.0)
        .bind(candidate_owner.id.0)
        .execute(&mut *transaction)
        .await?;
    sqlx::query(
        "UPDATE tracks SET output_path=NULL,selected_candidate_id=?,status='selected',stage='ready',
         is_missing=0,error=NULL,stage_message='Promoted as the best available duplicate input',
         updated_at=? WHERE id=?",
    )
    .bind(candidate_id.0)
    .bind(&now)
    .bind(representative.id.0)
    .execute(&mut *transaction)
    .await?;
    if owner_available {
        sqlx::query(
            "UPDATE tracks SET output_path=?,selected_candidate_id=NULL,status='duplicate',
             stage='skipped',error=NULL,stage_message=?,updated_at=? WHERE id=?",
        )
        .bind(&representative.path)
        .bind(format!("Duplicate of input file: {}", representative.path))
        .bind(&now)
        .bind(candidate_owner.id.0)
        .execute(&mut *transaction)
        .await?;
    } else {
        sqlx::query("UPDATE tracks SET selected_candidate_id=NULL,updated_at=? WHERE id=?")
            .bind(&now)
            .bind(candidate_owner.id.0)
            .execute(&mut *transaction)
            .await?;
    }
    transaction.commit().await?;
    Ok(())
}

pub(crate) async fn apply_ready_automatically(s: Arc<AppState>) -> Result<usize> {
    if s.frontend_active_until().await.is_some() {
        return Ok(0);
    }
    let prepared = prepare_apply(&s).await?;
    if prepared.selected_count == 0 {
        return Ok(0);
    }
    let count = prepared.outputs;
    if !s
        .try_claim_workflow(
            WorkflowPhase::Apply,
            "Writing corrected copies automatically",
            true,
        )
        .await
    {
        return Ok(0);
    }
    if s.frontend_active_until().await.is_some() || s.workflow_cancelled().await {
        s.finish_workflow(
            WorkflowPhase::Idle,
            "idle",
            "Automatic write paused while the web app is open",
        )
        .await;
        return Ok(0);
    }
    let result = apply(s.clone(), prepared.items).await;
    if s.workflow_cancelled().await {
        s.finish_workflow(WorkflowPhase::Idle, "idle", "Automatic write stopped")
            .await;
    } else if let Err(error) = &result {
        s.finish_workflow(WorkflowPhase::Failed, "failed", error.to_string())
            .await;
    } else {
        s.finish_workflow(
            WorkflowPhase::Finish,
            "finish",
            format!(
                "Automatic cleaning complete · {count} {} written",
                if count == 1 { "track" } else { "tracks" }
            ),
        )
        .await;
    }
    result?;
    Ok(count)
}

pub(crate) async fn finish_apply_workflow(
    state: Arc<AppState>,
    items: Vec<PreviewItem>,
) {
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
}

async fn recording_evidence(
    pool: &sqlx::SqlitePool,
    track: &Track,
    candidate: Option<&Candidate>,
) -> Result<RecordingEvidence> {
    let path = std::path::Path::new(&track.path);
    let content_key = if track.content_fingerprint.is_some() {
        track.content_fingerprint.clone()
    } else if let Some(value) = fingerprint_cache::cached(pool, path).await? {
        input_dedup::fingerprint_key(&value.fingerprint)
    } else {
        Some(input_dedup::hash_key(
            &input_dedup::sha256_cached(pool, path).await?,
        ))
    };
    Ok(RecordingEvidence {
        path: path.to_path_buf(),
        format: track.format.clone().unwrap_or_default(),
        bitrate: track.bitrate.and_then(|value| u32::try_from(value).ok()),
        duration: track.duration,
        content_key,
        isrc: candidate.and_then(|candidate| candidate.isrc.clone()),
    })
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

#[derive(Debug, Eq, PartialEq)]
enum Publication {
    Written(PathBuf),
    Reused(PathBuf),
}

impl Publication {
    fn path(&self) -> &std::path::Path {
        match self {
            Self::Written(path) | Self::Reused(path) => path,
        }
    }

    fn reused_existing(&self) -> bool {
        matches!(self, Self::Reused(_))
    }
}

fn destination_variant_number(
    preferred: &std::path::Path,
    candidate: &std::path::Path,
) -> Option<usize> {
    if candidate.file_name() == preferred.file_name() {
        return Some(1);
    }
    if candidate.extension() != preferred.extension() {
        return None;
    }
    let preferred_stem = preferred.file_stem()?.to_str()?;
    let candidate_stem = candidate.file_stem()?.to_str()?;
    let number = candidate_stem
        .strip_prefix(preferred_stem)?
        .strip_prefix(" (")?
        .strip_suffix(')')?
        .parse::<usize>()
        .ok()?;
    (number >= 2).then_some(number)
}

async fn existing_destination_variants(preferred: &std::path::Path) -> Result<Vec<PathBuf>> {
    let parent = preferred
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| std::path::Path::new("."));
    let mut directory = match tokio::fs::read_dir(parent).await {
        Ok(directory) => directory,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(Vec::new()),
        Err(error) => return Err(error.into()),
    };
    let mut variants = Vec::new();
    while let Some(entry) = directory.next_entry().await? {
        let path = entry.path();
        if let Some(number) = destination_variant_number(preferred, &path)
            && tokio::fs::metadata(&path)
                .await
                .is_ok_and(|metadata| metadata.is_file())
        {
            variants.push((number, path));
        }
    }
    variants.sort_by_key(|(number, _)| *number);
    Ok(variants.into_iter().map(|(_, path)| path).collect())
}

struct AudioEquivalenceProbe {
    file_size: u64,
    file_hash: String,
    fingerprint: Option<Option<(String, f64)>>,
}

async fn files_are_equivalent_audio(
    temporary: &std::path::Path,
    existing: &std::path::Path,
    probe: &mut Option<AudioEquivalenceProbe>,
) -> Result<bool> {
    if probe.is_none() {
        let metadata = tokio::fs::metadata(temporary).await?;
        *probe = Some(AudioEquivalenceProbe {
            file_size: metadata.len(),
            file_hash: file_sha256(temporary).await?,
            fingerprint: None,
        });
    }
    let probe = probe.as_mut().expect("temporary probe initialized");
    let existing_metadata = tokio::fs::metadata(existing).await?;
    if !existing_metadata.is_file() {
        return Ok(false);
    }
    if probe.file_size == existing_metadata.len() {
        let existing_hash = file_sha256(existing).await?;
        if probe.file_hash == existing_hash {
            return Ok(true);
        }
    }

    if probe.fingerprint.is_none() {
        probe.fingerprint = Some(fingerprint::calculate(temporary).await.ok());
    }
    let (
        Some((temporary_fingerprint, temporary_duration)),
        Ok((existing_fingerprint, existing_duration)),
    ) = (
        probe.fingerprint.as_ref().and_then(Option::as_ref),
        fingerprint::calculate(existing).await,
    )
    else {
        return Ok(false);
    };
    Ok(temporary_fingerprint == &existing_fingerprint
        && (temporary_duration - existing_duration).abs() <= 3.0)
}

async fn discard_temporary(temporary: &std::path::Path) {
    if let Err(error) = tokio::fs::remove_file(temporary).await {
        tracing::warn!(
            path = %temporary.display(),
            %error,
            "reused existing output but could not remove temporary file"
        );
    }
}

async fn paths_are_same_existing_file(left: &std::path::Path, right: &std::path::Path) -> bool {
    tokio::try_join!(
        tokio::fs::canonicalize(left),
        tokio::fs::canonicalize(right)
    )
    .is_ok_and(|(left, right)| left == right)
}

/// Reuses an equivalent existing output instead of creating another numbered
/// copy. A genuinely different recording is still published without replacing
/// anything already present. Hard links keep the final publication atomic.
///
/// An existing output is only reused when it also carries the exact artwork we
/// are about to publish, so a stale file with missing or wrong cover art is
/// never silently accepted as the "corrected" output.
async fn publish_no_clobber(
    temporary: &std::path::Path,
    preferred: &std::path::Path,
    excluded_source: Option<&std::path::Path>,
    expected_artwork: Option<&[u8]>,
) -> Result<Publication> {
    tokio::fs::OpenOptions::new()
        .write(true)
        .open(temporary)
        .await?
        .sync_all()
        .await?;

    let mut equivalence_probe = None;
    for existing in existing_destination_variants(preferred).await? {
        if let Some(source) = excluded_source
            && paths_are_same_existing_file(source, &existing).await
        {
            continue;
        }
        if files_are_equivalent_audio(temporary, &existing, &mut equivalence_probe).await?
            && existing_file_has_expected_artwork(&existing, expected_artwork).await?
        {
            discard_temporary(temporary).await;
            return Ok(Publication::Reused(existing));
        }
    }

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
                return Ok(Publication::Written(destination));
            }
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => {
                if let Some(source) = excluded_source
                    && paths_are_same_existing_file(source, &destination).await
                {
                    number += 1;
                    continue;
                }
                if files_are_equivalent_audio(temporary, &destination, &mut equivalence_probe)
                    .await?
                    && existing_file_has_expected_artwork(&destination, expected_artwork).await?
                {
                    discard_temporary(temporary).await;
                    return Ok(Publication::Reused(destination));
                }
                number += 1;
            }
            Err(error) => return Err(error.into()),
        }
    }
}

/// A reusable existing output must already contain the exact cover we are about
/// to write. When no artwork is expected (defensive path) any existing output
/// passes this check.
async fn existing_file_has_expected_artwork(
    existing: &std::path::Path,
    expected: Option<&[u8]>,
) -> Result<bool> {
    let Some(expected) = expected else {
        return Ok(true);
    };
    let existing = existing.to_path_buf();
    let expected = expected.to_vec();
    let matches = tokio::task::spawn_blocking(move || {
        match crate::infrastructure::media::tag_writer::read_artwork(&existing) {
            Ok(Some(bytes)) => bytes == expected,
            _ => false,
        }
    })
    .await?;
    Ok(matches)
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
        let candidate = item.candidate.clone();
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
        let artwork = match resolve_artwork(&s, &item.filename, &candidate).await {
            Ok(artwork) => artwork,
            Err(error) => {
                return_track_for_cover(&s, item.track_id, &error).await?;
                continue;
            }
        };
        let src = PathBuf::from(&item.current_path);
        let dest = PathBuf::from(&item.destination_path);
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
        let expected_artwork = artwork.clone();
        let result = tokio::task::spawn_blocking(move || {
            let _permit = write_permit;
            let sanitized = tag_writer::write_resilient(
                &write_target,
                &candidate,
                Some(artwork.clone()),
                replay_gain,
            )?;
            tag_writer::verify_written_metadata(&write_target, &candidate)?;
            tag_writer::verify_embedded_artwork(&write_target, &artwork)?;
            Ok::<_, anyhow::Error>(sanitized)
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
            Ok(_) => {
                publish_no_clobber(&temporary, &dest, Some(&src), Some(&expected_artwork)).await
            }
            Err(error) => Err(error),
        };
        let output_available = publication.is_ok();
        let reused_existing = publication.as_ref().is_ok_and(Publication::reused_existing);
        let final_path = publication
            .as_ref()
            .map(|publication| publication.path().to_owned())
            .unwrap_or_else(|_| dest.clone());
        let mut result = publication.map(|_| ());
        let (status, error) = match result {
            Ok(_) => ("applied", None),
            Err(e) => {
                s.log_entry(
                    ActivityLogEntry::new(
                        "error",
                        "apply",
                        if output_available {
                            "Corrected output is available, but original removal failed"
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
        .bind(output_available.then(|| final_path.to_string_lossy().into_owned()))
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
                ActivityLogEntry::new(
                    "ok",
                    if reused_existing {
                        "deduplicate"
                    } else {
                        "apply"
                    },
                    if reused_existing {
                        "Skipped duplicate output; equivalent corrected file already exists"
                    } else {
                        "Applied metadata changes"
                    },
                )
                .file(item.filename.clone())
                .context(serde_json::json!({
                    "output": final_path.display().to_string(),
                    "source_removed": false,
                    "reused_existing": reused_existing
                })),
            )
            .await;
            for duplicate in &item.duplicates {
                if let Err(error) =
                    finish_duplicate(&s, duplicate, &final_path).await
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

async fn return_track_for_cover(
    state: &Arc<AppState>,
    track_id: crate::types::TrackId,
    error: &anyhow::Error,
) -> Result<()> {
    let detail = format!("Cover verification failed before writing: {error:#}");
    sqlx::query(
        "UPDATE tracks SET status='needs_review',stage='review',stage_message=?,error=NULL,updated_at=? WHERE id=?",
    )
    .bind(&detail)
    .bind(Utc::now().to_rfc3339())
    .bind(track_id.0)
    .execute(&state.pool)
    .await?;
    state
        .log_entry(
            ActivityLogEntry::new("warn", "artwork", "Track returned to Review").detail(detail),
        )
        .await;
    Ok(())
}

async fn finish_duplicate(
    state: &Arc<AppState>,
    duplicate: &DuplicateSource,
    output: &std::path::Path,
) -> Result<()> {
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
                "source_removed": false,
                "source_missing": duplicate.source_missing
            })),
        )
        .await;
    Ok(())
}

/// Fetch the exact verified artwork bytes to embed, or fail so the caller can
/// return the track to review. Artwork is never optional at write time: a file
/// is only published when it has valid matching cover art. Only artwork that
/// matches the selected release is considered.
pub(crate) async fn resolve_artwork(
    state: &Arc<AppState>,
    filename: &str,
    candidate: &crate::infrastructure::providers::Candidate,
) -> Result<Vec<u8>> {
    let limiter = state.artwork_downloads.read().await.clone();
    let _permit = limiter.acquire_owned().await?;
    match crate::application::artwork::resolve_for_write(&state.pool, &state.client, candidate)
        .await
    {
        Ok(bytes) => {
            state
                .log_entry(
                    ActivityLogEntry::new("ok", "artwork", "Downloaded verified cover art")
                        .file(filename.to_owned())
                        .context(serde_json::json!({
                            "provider": candidate.provider,
                            "url": candidate.cover_url,
                        })),
                )
                .await;
            Ok(bytes)
        }
        Err(error) => {
            state
                .log_entry(
                    ActivityLogEntry::new(
                        "warn",
                        "artwork",
                        "No matching cover artwork is available; track returned to review",
                    )
                    .file(filename.to_owned())
                    .error_text(format!("{error:#}")),
                )
                .await;
            Err(error)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn other_artwork_png() -> Vec<u8> {
        let image = image::RgbaImage::from_fn(320, 320, |x, y| {
            if (x / 25 + y / 25) % 3 == 0 {
                image::Rgba([190, 40, 80, 255])
            } else if (x / 25 + y / 25) % 3 == 1 {
                image::Rgba([40, 190, 120, 255])
            } else {
                image::Rgba([90, 70, 220, 255])
            }
        });
        let mut output = std::io::Cursor::new(Vec::new());
        image::DynamicImage::ImageRgba8(image)
            .write_to(&mut output, image::ImageFormat::Png)
            .unwrap();
        output.into_inner()
    }

    #[tokio::test]
    async fn automatic_write_excludes_tracks_that_are_still_in_review() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("review-write.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let track_id = sqlx::query("INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage) VALUES('/music/review.mp3','review.mp3','needs_review',0,'now','now','now','review')")
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
        let candidate_id = sqlx::query("INSERT INTO candidates(track_id,provider,title,artist,score) VALUES(?,'deezer','Song','Artist',80)")
            .bind(track_id)
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
        sqlx::query("UPDATE tracks SET selected_candidate_id=? WHERE id=?")
            .bind(candidate_id)
            .bind(track_id)
            .execute(&pool)
            .await
            .unwrap();
        let state = Arc::new(AppState::new(
            crate::config::Config::default(),
            pool.clone(),
        ));

        let prepared = prepare_apply(&state).await.unwrap();

        assert_eq!(prepared.selected_count, 0);
        assert!(prepared.items.is_empty());
    }

    #[tokio::test]
    async fn prepared_write_includes_early_input_duplicates_in_one_output() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("input-duplicates.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let input = directory.path().join("input");
        let output = directory.path().join("output");
        tokio::fs::create_dir_all(&input).await.unwrap();
        tokio::fs::create_dir_all(&output).await.unwrap();
        let kept_path = input.join("kept.flac");
        let duplicate_path = input.join("duplicate.mp3");
        tokio::fs::write(&kept_path, b"kept").await.unwrap();
        tokio::fs::write(&duplicate_path, b"duplicate")
            .await
            .unwrap();
        let kept_id = sqlx::query(
            "INSERT INTO tracks(path,filename,format,bitrate,duration,content_fingerprint,status,
             is_missing,first_seen_at,last_seen_at,last_scanned_at,stage)
             VALUES(?,'kept.flac','flac',900,180.0,'fp:same','selected',0,'now','now','now','ready')",
        )
        .bind(kept_path.to_string_lossy().as_ref())
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
        let candidate_id = sqlx::query(
            "INSERT INTO candidates(track_id,provider,title,artist,isrc,score)
             VALUES(?,'deezer','Song','Artist',NULL,100)",
        )
        .bind(kept_id)
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
        sqlx::query("UPDATE tracks SET selected_candidate_id=? WHERE id=?")
            .bind(candidate_id)
            .bind(kept_id)
            .execute(&pool)
            .await
            .unwrap();
        sqlx::query(
            "INSERT INTO tracks(path,filename,format,bitrate,duration,content_fingerprint,status,
             is_missing,first_seen_at,last_seen_at,last_scanned_at,stage)
             VALUES(?,'duplicate.mp3','mp3',320,181.0,'fp:same','duplicate',0,'now','now','now','skipped')",
        )
        .bind(duplicate_path.to_string_lossy().as_ref())
        .execute(&pool)
        .await
        .unwrap();
        let state = Arc::new(AppState::new(
            crate::config::Config {
                input_dir: input.to_string_lossy().into_owned(),
                output_dir: output.to_string_lossy().into_owned(),
                ..Default::default()
            },
            pool,
        ));

        let prepared = prepare_apply(&state).await.unwrap();

        assert_eq!(prepared.selected_count, 2);
        assert_eq!(prepared.outputs, 1);
        assert_eq!(prepared.duplicates_skipped, 1);
        assert_eq!(prepared.items[0].current_path, kept_path.to_string_lossy());
        assert_eq!(prepared.items[0].duplicates.len(), 1);
    }

    #[tokio::test]
    async fn prepared_write_promotes_an_available_duplicate_when_the_selected_source_is_missing() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("missing-representative.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let input = directory.path().join("input");
        let output = directory.path().join("output");
        tokio::fs::create_dir_all(&input).await.unwrap();
        tokio::fs::create_dir_all(&output).await.unwrap();
        let missing_path = input.join("missing.flac");
        let promoted_path = input.join("available.mp3");
        tokio::fs::write(&promoted_path, b"available")
            .await
            .unwrap();
        let missing_id = sqlx::query(
            "INSERT INTO tracks(path,filename,format,bitrate,duration,content_fingerprint,status,
             is_missing,first_seen_at,last_seen_at,last_scanned_at,stage)
             VALUES(?,'missing.flac','flac',900,180.0,'fp:same','selected',0,'now','now','now','ready')",
        )
        .bind(missing_path.to_string_lossy().as_ref())
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
        let candidate_id = sqlx::query(
            "INSERT INTO candidates(track_id,provider,title,artist,score)
             VALUES(?,'deezer','Song','Artist',100)",
        )
        .bind(missing_id)
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
        sqlx::query("UPDATE tracks SET selected_candidate_id=? WHERE id=?")
            .bind(candidate_id)
            .bind(missing_id)
            .execute(&pool)
            .await
            .unwrap();
        let promoted_id = sqlx::query(
            "INSERT INTO tracks(path,filename,format,bitrate,duration,content_fingerprint,status,
             is_missing,first_seen_at,last_seen_at,last_scanned_at,stage)
             VALUES(?,'available.mp3','mp3',320,180.0,'fp:same','duplicate',0,'now','now','now','skipped')",
        )
        .bind(promoted_path.to_string_lossy().as_ref())
        .execute(&pool)
        .await
        .unwrap()
        .last_insert_rowid();
        let state = Arc::new(AppState::new(
            crate::config::Config {
                input_dir: input.to_string_lossy().into_owned(),
                output_dir: output.to_string_lossy().into_owned(),
                ..Default::default()
            },
            pool.clone(),
        ));

        let prepared = prepare_apply(&state).await.unwrap();

        assert_eq!(prepared.outputs, 1);
        assert_eq!(prepared.items[0].track_id.0, promoted_id);
        assert_eq!(
            prepared.items[0].current_path,
            promoted_path.to_string_lossy()
        );
        assert!(prepared.items[0].duplicates[0].source_missing);
        assert_eq!(
            sqlx::query_scalar::<_, i64>("SELECT track_id FROM candidates WHERE id=?")
                .bind(candidate_id)
                .fetch_one(&pool)
                .await
                .unwrap(),
            promoted_id
        );
    }

    #[test]
    fn recognizes_only_numbered_variants_of_the_preferred_destination() {
        let base = std::path::Path::new("/output/Artist - Song.mp3");
        assert_eq!(destination_variant_number(base, base), Some(1));
        assert_eq!(
            destination_variant_number(base, std::path::Path::new("/output/Artist - Song (2).mp3")),
            Some(2)
        );
        assert_eq!(
            destination_variant_number(
                base,
                std::path::Path::new("/output/Artist - Song Remix (2).mp3")
            ),
            None
        );
        assert_eq!(
            destination_variant_number(
                base,
                std::path::Path::new("/output/Artist - Song (2).flac")
            ),
            None
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

        let published = publish_no_clobber(&temporary, &preferred, None, None)
            .await
            .unwrap();

        assert_eq!(
            published,
            Publication::Written(directory.path().join("Artist - Song (2).mp3"))
        );
        assert_eq!(
            tokio::fs::read(&preferred).await.unwrap(),
            b"existing output"
        );
        assert_eq!(
            tokio::fs::read(published.path()).await.unwrap(),
            b"new output"
        );
        assert!(!temporary.exists());
    }

    #[tokio::test]
    async fn equivalent_existing_output_is_reused_without_numbering() {
        let directory = tempfile::tempdir().unwrap();
        let temporary = directory.path().join(".temporary.mp3");
        let preferred = directory.path().join("Artist - Song.mp3");
        tokio::fs::write(&temporary, b"same corrected audio")
            .await
            .unwrap();
        tokio::fs::write(&preferred, b"same corrected audio")
            .await
            .unwrap();

        let published = publish_no_clobber(&temporary, &preferred, None, None)
            .await
            .unwrap();

        assert_eq!(published, Publication::Reused(preferred));
        assert!(!temporary.exists());
        assert!(!directory.path().join("Artist - Song (2).mp3").exists());
    }

    #[tokio::test]
    async fn audio_fingerprint_reuses_existing_output_with_different_tags() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
            || std::process::Command::new("fpcalc")
                .arg("-version")
                .output()
                .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let temporary = directory.path().join(".temporary.mp3");
        let preferred = directory.path().join("Artist - Song.mp3");
        let generated = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=4")
            .args(["-q:a", "4", "-metadata", "title=Existing"])
            .arg(&preferred)
            .status()
            .unwrap();
        assert!(generated.success());
        let retagged = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-i"])
            .arg(&preferred)
            .args([
                "-map",
                "0:a:0",
                "-c:a",
                "copy",
                "-metadata",
                "title=Retagged",
            ])
            .arg(&temporary)
            .status()
            .unwrap();
        assert!(retagged.success());
        assert_ne!(
            file_sha256(&temporary).await.unwrap(),
            file_sha256(&preferred).await.unwrap()
        );

        let published = publish_no_clobber(&temporary, &preferred, None, None)
            .await
            .unwrap();

        assert_eq!(published, Publication::Reused(preferred));
        assert!(!temporary.exists());
        assert!(!directory.path().join("Artist - Song (2).mp3").exists());
    }

    #[tokio::test]
    async fn existing_output_with_different_artwork_is_not_reused() {
        if std::process::Command::new("ffmpeg")
            .arg("-version")
            .output()
            .is_err()
        {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("source.mp3");
        let existing = directory.path().join("Artist - Song.mp3");
        let temporary = directory.path().join(".temporary.mp3");
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=4")
            .args(["-q:a", "4"])
            .arg(&source)
            .status()
            .unwrap();
        assert!(status.success());
        tokio::fs::copy(&source, &existing).await.unwrap();
        tokio::fs::copy(&source, &temporary).await.unwrap();
        let candidate = crate::infrastructure::providers::Candidate {
            title: "Song".into(),
            artist: "Artist".into(),
            ..Default::default()
        };
        let artwork_a = crate::infrastructure::media::tag_writer::test_artwork_png();
        let artwork_b = other_artwork_png();
        crate::infrastructure::media::tag_writer::write(
            &existing,
            &candidate,
            Some(artwork_a.clone()),
            None,
        )
        .unwrap();
        crate::infrastructure::media::tag_writer::write(
            &temporary,
            &candidate,
            Some(artwork_b.clone()),
            None,
        )
        .unwrap();

        let published = publish_no_clobber(&temporary, &existing, None, Some(&artwork_b))
            .await
            .unwrap();

        assert_eq!(
            published,
            Publication::Written(directory.path().join("Artist - Song (2).mp3"))
        );
        assert!(!temporary.exists());
        assert_eq!(
            crate::infrastructure::media::tag_writer::read_artwork(published.path()).unwrap(),
            Some(artwork_b)
        );
        assert_eq!(
            crate::infrastructure::media::tag_writer::read_artwork(&existing).unwrap(),
            Some(artwork_a)
        );
    }

    #[tokio::test]
    async fn reused_output_keeps_source_file() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("input.mp3");
        let temporary = directory.path().join(".temporary.mp3");
        let preferred = directory.path().join("Artist - Song.mp3");
        tokio::fs::write(&source, b"same corrected audio")
            .await
            .unwrap();
        tokio::fs::copy(&source, &temporary).await.unwrap();
        tokio::fs::copy(&source, &preferred).await.unwrap();

        let published = publish_no_clobber(&temporary, &preferred, None, None)
            .await
            .unwrap();
        assert!(published.reused_existing());
        assert!(source.exists());
        assert!(preferred.exists());
        assert!(!directory.path().join("Artist - Song (2).mp3").exists());
    }

    #[tokio::test]
    async fn source_file_is_not_mistaken_for_an_existing_corrected_output() {
        let directory = tempfile::tempdir().unwrap();
        let source = directory.path().join("Artist - Song.mp3");
        let existing_output = directory.path().join("Artist - Song (2).mp3");
        let temporary = directory.path().join(".temporary.mp3");
        tokio::fs::write(&source, b"same audio").await.unwrap();
        tokio::fs::copy(&source, &existing_output).await.unwrap();
        tokio::fs::copy(&source, &temporary).await.unwrap();

        let published = publish_no_clobber(&temporary, &source, Some(&source), None)
            .await
            .unwrap();

        assert_eq!(published, Publication::Reused(existing_output));
        assert!(source.exists());
        assert!(!temporary.exists());
    }

    #[test]
    fn temporary_destination_keeps_audio_extension() {
        assert_eq!(
            temporary_destination(std::path::Path::new("/output/Artist - Song.mp3"), 42),
            PathBuf::from("/output/.Artist - Song.ununknown-42.mp3")
        );
    }

    #[tokio::test]
    async fn duplicate_source_is_kept_after_the_output_exists() {
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
            source_missing: false,
        };

        finish_duplicate(&state, &duplicate, &output).await.unwrap();

        assert!(source.exists());
        assert!(output.exists());
        let remaining: i64 = sqlx::query_scalar("SELECT count(*) FROM tracks WHERE id=?")
            .bind(track_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(remaining, 0);
    }

}
