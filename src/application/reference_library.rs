use crate::{
    app::AppState, domain::audio, infrastructure::media::fingerprint, types::WorkflowPhase,
};
use anyhow::{Context, Result, bail};
use chrono::Utc;
use serde::Serialize;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::{
    collections::{HashMap, HashSet},
    path::{Path, PathBuf},
    sync::Arc,
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::{io::AsyncReadExt, task::JoinSet};
use walkdir::WalkDir;

#[derive(Clone, Debug, Default, Serialize)]
pub struct IndexStats {
    pub files: usize,
    pub updated: usize,
    pub reused: usize,
    pub failed: usize,
}

#[derive(Clone, Debug)]
pub struct ReferenceMatch {
    pub path: String,
    pub reason: &'static str,
}

/// Enforces the read-only boundary even when paths came from environment
/// variables instead of the settings API.
pub async fn validate_layout(config: &crate::config::Config) -> Result<()> {
    let input = tokio::fs::canonicalize(&config.input_dir).await.ok();
    let output = tokio::fs::canonicalize(&config.output_dir).await.ok();
    for configured in &config.reference_dirs {
        let Ok(reference) = tokio::fs::canonicalize(configured).await else {
            continue;
        };
        if input
            .as_deref()
            .is_some_and(|input| paths_overlap(&reference, input))
        {
            bail!("reference folder must not overlap the input folder: {configured}");
        }
        if output
            .as_deref()
            .is_some_and(|output| paths_overlap(&reference, output))
        {
            bail!("reference folder must not overlap the output folder: {configured}");
        }
    }
    Ok(())
}

#[derive(Clone)]
struct DiscoveredFile {
    path: PathBuf,
    root: String,
    size: i64,
    modified_ns: i64,
}

struct IndexedFile {
    discovered: DiscoveredFile,
    duration: Option<f64>,
    fingerprint: Option<String>,
    file_hash: Option<String>,
    error: Option<String>,
}

/// Refreshes the local manifest for folders that Ununknown is only allowed to
/// read. Unchanged files never invoke fpcalc again.
pub async fn refresh(state: &Arc<AppState>) -> Result<IndexStats> {
    let config = state.config.read().await.clone();
    if config.reference_dirs.is_empty() {
        sqlx::query("DELETE FROM reference_files")
            .execute(&state.pool)
            .await?;
        return Ok(IndexStats::default());
    }
    validate_layout(&config).await?;

    state
        .set_workflow(
            WorkflowPhase::Scan,
            "reference_index",
            "Checking read-only reference libraries",
            0,
            0,
            None,
        )
        .await;
    let roots = config.reference_dirs.clone();
    let (files, walk_errors) = tokio::task::spawn_blocking(move || discover(&roots))
        .await
        .context("reference-library discovery task failed")?;
    let discovery_complete = walk_errors.is_empty();
    for error in walk_errors {
        state.log("warn", "reference_index", None, &error).await;
    }

    let existing: Vec<(String, String, i64, i64)> = sqlx::query_as(
        "SELECT path,root,file_size,file_mtime_ns FROM reference_files ORDER BY path",
    )
    .fetch_all(&state.pool)
    .await?;
    let existing = existing
        .into_iter()
        .map(|(path, root, size, modified)| (path, (root, size, modified)))
        .collect::<HashMap<_, _>>();
    let current_paths = files
        .iter()
        .map(|file| file.path.to_string_lossy().into_owned())
        .collect::<HashSet<_>>();
    let changed = files
        .iter()
        .filter(|file| {
            existing
                .get(file.path.to_string_lossy().as_ref())
                .is_none_or(|(root, size, modified)| {
                    root != &file.root || (*size, *modified) != (file.size, file.modified_ns)
                })
        })
        .cloned()
        .collect::<Vec<_>>();

    let total = files.len();
    let changed_count = changed.len();
    state
        .set_workflow(
            WorkflowPhase::Scan,
            "reference_index",
            if changed_count == 0 {
                "Reference-library index is current"
            } else {
                "Fingerprinting changed reference music"
            },
            total.saturating_sub(changed_count),
            total,
            None,
        )
        .await;

    let workers = config.fingerprint_workers.max(1);
    let mut tasks = JoinSet::new();
    let mut changed_iter = changed.into_iter();
    let mut completed = 0_usize;
    loop {
        if state.workflow_cancelled().await {
            tasks.abort_all();
            break;
        }
        while tasks.len() < workers {
            let Some(file) = changed_iter.next() else {
                break;
            };
            tasks.spawn(async move { index_one(file).await });
        }
        let Some(result) = tasks.join_next().await else {
            break;
        };
        let indexed = result.context("reference fingerprint worker failed")?;
        upsert(&state.pool, &indexed).await?;
        completed += 1;
        state
            .set_workflow(
                WorkflowPhase::Scan,
                "reference_index",
                "Fingerprinting changed reference music",
                total.saturating_sub(changed_count) + completed,
                total,
                Some(indexed.discovered.path.to_string_lossy().into_owned()),
            )
            .await;
    }

    // A disconnected NAS or unreadable subtree must not erase a valid cached
    // index. Missing entries are pruned only after a completely clean walk.
    let configured_roots = config.reference_dirs.iter().collect::<HashSet<_>>();
    let stale = existing
        .iter()
        .filter(|(path, (root, _, _))| {
            !configured_roots.contains(root)
                || discovery_complete && !current_paths.contains(path.as_str())
        })
        .map(|(path, _)| path.clone())
        .collect::<Vec<_>>();
    if !stale.is_empty() {
        let mut transaction = state.pool.begin().await?;
        for path in stale {
            sqlx::query("DELETE FROM reference_files WHERE path=?")
                .bind(path)
                .execute(&mut *transaction)
                .await?;
        }
        transaction.commit().await?;
    }

    let failed = sqlx::query_scalar::<_, i64>(
        "SELECT COUNT(*) FROM reference_files WHERE error IS NOT NULL",
    )
    .fetch_one(&state.pool)
    .await?
    .max(0) as usize;
    let stats = IndexStats {
        files: total,
        updated: changed_count,
        reused: total.saturating_sub(changed_count),
        failed,
    };
    state
        .log(
            if failed == 0 { "ok" } else { "warn" },
            "reference_index",
            None,
            &format!(
                "Reference index: {} files, {} updated, {} unchanged, {} unavailable",
                stats.files, stats.updated, stats.reused, stats.failed
            ),
        )
        .await;
    Ok(stats)
}

pub async fn stats(pool: &SqlitePool) -> Result<IndexStats> {
    let (files, failed): (i64, i64) =
        sqlx::query_as("SELECT COUNT(*), COALESCE(SUM(error IS NOT NULL), 0) FROM reference_files")
            .fetch_one(pool)
            .await?;
    Ok(IndexStats {
        files: files.max(0) as usize,
        failed: failed.max(0) as usize,
        ..Default::default()
    })
}

pub async fn mark_existing_track(
    pool: &SqlitePool,
    track_id: i64,
    found: &ReferenceMatch,
) -> Result<()> {
    sqlx::query(
        "UPDATE tracks SET output_path=?,status='duplicate',stage='skipped',
         stage_message=?,error=NULL,selected_candidate_id=NULL,updated_at=? WHERE id=?",
    )
    .bind(&found.path)
    .bind(format!(
        "Already exists in read-only library ({}): {}",
        found.reason, found.path
    ))
    .bind(Utc::now().to_rfc3339())
    .bind(track_id)
    .execute(pool)
    .await?;
    Ok(())
}

/// Finds the same recording in a reference folder. Fingerprints catch tag,
/// container, and bitrate differences; SHA-256 is a fallback for files fpcalc
/// could not analyze.
pub async fn find_duplicate(
    pool: &SqlitePool,
    source: &Path,
    fingerprint_value: Option<&str>,
    duration: f64,
) -> Result<Option<ReferenceMatch>> {
    if let Some(value) = fingerprint_value.filter(|value| !value.is_empty()) {
        let paths: Vec<String> = sqlx::query_scalar(
            "SELECT path FROM reference_files
             WHERE fingerprint=? AND ABS(duration - ?) <= 3.0
             ORDER BY path",
        )
        .bind(value)
        .bind(duration)
        .fetch_all(pool)
        .await?;
        for path in paths {
            if tokio::fs::metadata(&path)
                .await
                .is_ok_and(|metadata| metadata.is_file())
            {
                return Ok(Some(ReferenceMatch {
                    path,
                    reason: "audio fingerprint",
                }));
            }
        }
    }

    let size = tokio::fs::metadata(source).await?.len() as i64;
    let candidates: Vec<(String, Option<String>)> = if fingerprint_value.is_some() {
        sqlx::query_as(
            "SELECT path,file_hash FROM reference_files
             WHERE file_size=? AND fingerprint IS NULL ORDER BY path",
        )
        .bind(size)
        .fetch_all(pool)
        .await?
    } else {
        sqlx::query_as("SELECT path,file_hash FROM reference_files WHERE file_size=? ORDER BY path")
            .bind(size)
            .fetch_all(pool)
            .await?
    };
    if candidates.is_empty() {
        return Ok(None);
    }
    let source_hash = sha256(source).await?;
    for (path, cached_hash) in candidates {
        if !tokio::fs::metadata(&path)
            .await
            .is_ok_and(|metadata| metadata.is_file())
        {
            continue;
        }
        let reference_hash = match cached_hash {
            Some(hash) => hash,
            None => {
                let hash = match sha256(Path::new(&path)).await {
                    Ok(hash) => hash,
                    Err(_) => continue,
                };
                sqlx::query("UPDATE reference_files SET file_hash=? WHERE path=?")
                    .bind(&hash)
                    .bind(&path)
                    .execute(pool)
                    .await?;
                hash
            }
        };
        if source_hash == reference_hash {
            return Ok(Some(ReferenceMatch {
                path,
                reason: "exact file hash",
            }));
        }
    }
    Ok(None)
}

fn discover(roots: &[String]) -> (Vec<DiscoveredFile>, Vec<String>) {
    let mut files = HashMap::<PathBuf, DiscoveredFile>::new();
    let mut errors = Vec::new();
    for configured_root in roots {
        let root = match std::fs::canonicalize(configured_root) {
            Ok(root) => root,
            Err(error) => {
                errors.push(format!(
                    "Could not read reference folder {configured_root}: {error}"
                ));
                continue;
            }
        };
        let root_text = configured_root.clone();
        for entry in WalkDir::new(&root).follow_links(false) {
            let entry = match entry {
                Ok(entry) => entry,
                Err(error) => {
                    errors.push(format!("Reference folder walk error: {error}"));
                    continue;
                }
            };
            if !entry.file_type().is_file() || !audio::is_supported(entry.path()) {
                continue;
            }
            let metadata = match entry.metadata() {
                Ok(metadata) => metadata,
                Err(error) => {
                    errors.push(format!(
                        "Could not inspect {}: {error}",
                        entry.path().display()
                    ));
                    continue;
                }
            };
            let path = entry.path().to_path_buf();
            files.entry(path.clone()).or_insert(DiscoveredFile {
                path,
                root: root_text.clone(),
                size: i64::try_from(metadata.len()).unwrap_or(i64::MAX),
                modified_ns: modified_ns(metadata.modified().unwrap_or(UNIX_EPOCH)),
            });
        }
    }
    let mut files = files.into_values().collect::<Vec<_>>();
    files.sort_by(|left, right| left.path.cmp(&right.path));
    (files, errors)
}

async fn index_one(discovered: DiscoveredFile) -> IndexedFile {
    match fingerprint::calculate(&discovered.path).await {
        Ok((fingerprint, duration)) => IndexedFile {
            discovered,
            duration: Some(duration),
            fingerprint: Some(fingerprint),
            file_hash: None,
            error: None,
        },
        Err(error) => {
            let file_hash = sha256(&discovered.path).await.ok();
            IndexedFile {
                discovered,
                duration: None,
                fingerprint: None,
                file_hash,
                error: Some(format!("{error:#}")),
            }
        }
    }
}

async fn upsert(pool: &SqlitePool, file: &IndexedFile) -> Result<()> {
    sqlx::query(
        "INSERT INTO reference_files(path,root,file_size,file_mtime_ns,duration,fingerprint,file_hash,error,indexed_at)
         VALUES(?,?,?,?,?,?,?,?,?)
         ON CONFLICT(path) DO UPDATE SET
           root=excluded.root,file_size=excluded.file_size,file_mtime_ns=excluded.file_mtime_ns,
           duration=excluded.duration,fingerprint=excluded.fingerprint,file_hash=excluded.file_hash,
           error=excluded.error,indexed_at=excluded.indexed_at",
    )
    .bind(file.discovered.path.to_string_lossy().as_ref())
    .bind(&file.discovered.root)
    .bind(file.discovered.size)
    .bind(file.discovered.modified_ns)
    .bind(file.duration)
    .bind(&file.fingerprint)
    .bind(&file.file_hash)
    .bind(&file.error)
    .bind(Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(())
}

async fn sha256(path: &Path) -> Result<String> {
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

fn modified_ns(value: SystemTime) -> i64 {
    let nanos = value
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    i64::try_from(nanos).unwrap_or(i64::MAX)
}

fn paths_overlap(left: &Path, right: &Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn discovery_deduplicates_overlapping_roots() {
        let directory = tempfile::tempdir().unwrap();
        let nested = directory.path().join("nested");
        std::fs::create_dir_all(&nested).unwrap();
        std::fs::write(nested.join("song.mp3"), b"audio").unwrap();
        let roots = vec![
            directory.path().to_string_lossy().into_owned(),
            nested.to_string_lossy().into_owned(),
        ];
        let (files, errors) = discover(&roots);
        assert!(errors.is_empty());
        assert_eq!(files.len(), 1);
    }

    #[tokio::test]
    async fn exact_hash_fallback_finds_reference_file() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("reference.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let source = directory.path().join("source.mp3");
        let reference = directory.path().join("reference.mp3");
        tokio::fs::write(&source, b"identical audio").await.unwrap();
        tokio::fs::write(&reference, b"identical audio")
            .await
            .unwrap();
        sqlx::query("INSERT INTO reference_files(path,root,file_size,file_mtime_ns,indexed_at) VALUES(?,?,?,?,?)")
            .bind(reference.to_string_lossy().as_ref())
            .bind(directory.path().to_string_lossy().as_ref())
            .bind(15_i64)
            .bind(0_i64)
            .bind("now")
            .execute(&pool)
            .await
            .unwrap();

        let found = find_duplicate(&pool, &source, None, 1.0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.path, reference.to_string_lossy());
        assert_eq!(found.reason, "exact file hash");
    }

    #[tokio::test]
    async fn fingerprint_match_ignores_filename_and_file_size() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("fingerprint.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let source = directory.path().join("renamed.mp3");
        let reference = directory.path().join("original.flac");
        tokio::fs::write(&source, b"source placeholder")
            .await
            .unwrap();
        tokio::fs::write(&reference, b"different container size")
            .await
            .unwrap();
        sqlx::query("INSERT INTO reference_files(path,root,file_size,file_mtime_ns,duration,fingerprint,indexed_at) VALUES(?,?,?,?,?,?,?)")
            .bind(reference.to_string_lossy().as_ref())
            .bind(directory.path().to_string_lossy().as_ref())
            .bind(999_i64)
            .bind(0_i64)
            .bind(182.4_f64)
            .bind("same-audio-fingerprint")
            .bind("now")
            .execute(&pool)
            .await
            .unwrap();

        let found = find_duplicate(&pool, &source, Some("same-audio-fingerprint"), 180.0)
            .await
            .unwrap()
            .unwrap();
        assert_eq!(found.path, reference.to_string_lossy());
        assert_eq!(found.reason, "audio fingerprint");

        tokio::fs::remove_file(&reference).await.unwrap();
        assert!(
            find_duplicate(&pool, &source, Some("same-audio-fingerprint"), 180.0)
                .await
                .unwrap()
                .is_none()
        );
    }
}
