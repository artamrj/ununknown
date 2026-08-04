use anyhow::Result;
use chrono::Utc;
use sha2::{Digest, Sha256};
use sqlx::SqlitePool;
use std::{
    cmp::Ordering,
    collections::HashMap,
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::io::AsyncReadExt;

const DURATION_TOLERANCE_SECONDS: f64 = 3.0;

#[derive(Clone, Debug)]
pub struct RecordingEvidence {
    pub path: PathBuf,
    pub format: String,
    pub bitrate: Option<u32>,
    pub duration: Option<f64>,
    pub content_key: Option<String>,
    pub isrc: Option<String>,
}

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct DuplicateGroup {
    pub representative: usize,
    pub members: Vec<usize>,
}

pub fn fingerprint_key(value: &str) -> Option<String> {
    let value = value.trim();
    (!value.is_empty()).then(|| format!("fp:{}", hex::encode(Sha256::digest(value.as_bytes()))))
}

pub fn hash_key(value: &str) -> String {
    format!("sha256:{}", value.trim().to_ascii_lowercase())
}

pub fn normalize_isrc(value: &str) -> Option<String> {
    let normalized = value
        .chars()
        .filter(|character| character.is_ascii_alphanumeric())
        .flat_map(char::to_uppercase)
        .collect::<String>();
    (normalized.len() == 12).then_some(normalized)
}

/// Groups recordings using proof-only evidence. Exact hashes are definitive;
/// fingerprints and ISRCs additionally require compatible durations.
pub fn group_recordings(recordings: &[RecordingEvidence]) -> Vec<DuplicateGroup> {
    let mut sets = DisjointSet::new(recordings.len());
    let mut exact_hashes = HashMap::<&str, usize>::new();
    let mut fingerprints = HashMap::<&str, Vec<usize>>::new();
    let mut isrcs = HashMap::<String, Vec<usize>>::new();

    for (index, recording) in recordings.iter().enumerate() {
        if let Some(key) = recording.content_key.as_deref() {
            if key.starts_with("sha256:") {
                if let Some(existing) = exact_hashes.insert(key, index) {
                    sets.union(existing, index);
                }
            } else if key.starts_with("fp:") && recording.duration.is_some() {
                fingerprints.entry(key).or_default().push(index);
            }
        }
        if let Some(isrc) = recording.isrc.as_deref().and_then(normalize_isrc)
            && recording.duration.is_some()
        {
            isrcs.entry(isrc).or_default().push(index);
        }
    }

    for indices in fingerprints.values_mut().chain(isrcs.values_mut()) {
        union_duration_clusters(&mut sets, recordings, indices);
    }

    let mut groups = HashMap::<usize, Vec<usize>>::new();
    for index in 0..recordings.len() {
        groups.entry(sets.find(index)).or_default().push(index);
    }
    let mut groups = groups
        .into_values()
        .map(|mut members| {
            members.sort_by(|left, right| compare_quality(&recordings[*left], &recordings[*right]));
            DuplicateGroup {
                representative: members[0],
                members,
            }
        })
        .collect::<Vec<_>>();
    groups.sort_by(|left, right| {
        recordings[left.representative]
            .path
            .cmp(&recordings[right.representative].path)
    });
    groups
}

fn union_duration_clusters(
    sets: &mut DisjointSet,
    recordings: &[RecordingEvidence],
    indices: &mut [usize],
) {
    indices.sort_by(|left, right| {
        recordings[*left]
            .duration
            .partial_cmp(&recordings[*right].duration)
            .unwrap_or(Ordering::Equal)
            .then_with(|| recordings[*left].path.cmp(&recordings[*right].path))
    });
    let mut start = 0;
    while start < indices.len() {
        let anchor = indices[start];
        let anchor_duration = recordings[anchor].duration.unwrap_or_default();
        let mut end = start + 1;
        while end < indices.len()
            && recordings[indices[end]].duration.is_some_and(|duration| {
                (duration - anchor_duration).abs() <= DURATION_TOLERANCE_SECONDS
            })
        {
            sets.union(anchor, indices[end]);
            end += 1;
        }
        start = end;
    }
}

/// Best quality sorts first: lossless, then lossy bitrate, then stable path.
fn compare_quality(left: &RecordingEvidence, right: &RecordingEvidence) -> Ordering {
    let left_lossless = is_lossless(&left.format);
    let right_lossless = is_lossless(&right.format);
    right_lossless
        .cmp(&left_lossless)
        .then_with(|| {
            if left_lossless && right_lossless {
                Ordering::Equal
            } else {
                right
                    .bitrate
                    .unwrap_or_default()
                    .cmp(&left.bitrate.unwrap_or_default())
            }
        })
        .then_with(|| left.path.cmp(&right.path))
}

fn is_lossless(format: &str) -> bool {
    matches!(
        format.trim().to_ascii_lowercase().as_str(),
        "flac" | "wav" | "wave" | "aiff" | "aif" | "ape" | "wv" | "wavpack" | "alac"
    )
}

pub async fn sha256_cached(pool: &SqlitePool, path: &Path) -> Result<String> {
    let metadata = tokio::fs::metadata(path).await?;
    let size = i64::try_from(metadata.len()).unwrap_or(i64::MAX);
    let modified_ns = system_time_ns(metadata.modified()?);
    let path_text = path.to_string_lossy();
    if let Some(hash) = sqlx::query_scalar::<_, String>(
        "SELECT sha256 FROM content_hash_cache WHERE path=? AND file_size=? AND file_mtime_ns=?",
    )
    .bind(path_text.as_ref())
    .bind(size)
    .bind(modified_ns)
    .fetch_optional(pool)
    .await?
    {
        return Ok(hash);
    }

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
    let hash = hex::encode(digest.finalize());
    sqlx::query(
        "INSERT INTO content_hash_cache(path,file_size,file_mtime_ns,sha256,updated_at)
         VALUES(?,?,?,?,?)
         ON CONFLICT(path) DO UPDATE SET file_size=excluded.file_size,
           file_mtime_ns=excluded.file_mtime_ns,sha256=excluded.sha256,updated_at=excluded.updated_at",
    )
    .bind(path_text.as_ref())
    .bind(size)
    .bind(modified_ns)
    .bind(&hash)
    .bind(Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(hash)
}

fn system_time_ns(value: SystemTime) -> i64 {
    let nanos = value
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    i64::try_from(nanos).unwrap_or(i64::MAX)
}

struct DisjointSet {
    parents: Vec<usize>,
}

impl DisjointSet {
    fn new(len: usize) -> Self {
        Self {
            parents: (0..len).collect(),
        }
    }

    fn find(&mut self, index: usize) -> usize {
        if self.parents[index] != index {
            self.parents[index] = self.find(self.parents[index]);
        }
        self.parents[index]
    }

    fn union(&mut self, left: usize, right: usize) {
        let left = self.find(left);
        let right = self.find(right);
        if left != right {
            self.parents[right] = left;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn recording(
        path: &str,
        format: &str,
        bitrate: Option<u32>,
        duration: f64,
        content_key: Option<&str>,
        isrc: Option<&str>,
    ) -> RecordingEvidence {
        RecordingEvidence {
            path: path.into(),
            format: format.into(),
            bitrate,
            duration: Some(duration),
            content_key: content_key.map(str::to_owned),
            isrc: isrc.map(str::to_owned),
        }
    }

    #[test]
    fn fingerprint_groups_different_tags_but_not_different_durations() {
        let recordings = vec![
            recording("b.mp3", "mp3", Some(192), 180.0, Some("fp:same"), None),
            recording("a.mp3", "mp3", Some(320), 181.0, Some("fp:same"), None),
            recording("c.mp3", "mp3", Some(320), 240.0, Some("fp:same"), None),
        ];
        let groups = group_recordings(&recordings);
        assert_eq!(groups.len(), 2);
        assert_eq!(groups[0].representative, 1);
        assert_eq!(groups[0].members, vec![1, 0]);
    }

    #[test]
    fn exact_hash_is_definitive_and_lossless_wins() {
        let recordings = vec![
            recording("a.mp3", "mp3", Some(320), 10.0, Some("sha256:same"), None),
            recording("b.flac", "flac", Some(800), 99.0, Some("sha256:same"), None),
        ];
        let groups = group_recordings(&recordings);
        assert_eq!(groups.len(), 1);
        assert_eq!(groups[0].representative, 1);
    }

    #[test]
    fn only_valid_isrc_with_close_duration_groups() {
        let recordings = vec![
            recording("a.mp3", "mp3", None, 180.0, None, Some("US-ABC-12-34567")),
            recording("b.mp3", "mp3", None, 182.0, None, Some("usabc1234567")),
            recording("c.mp3", "mp3", None, 181.0, None, Some("invalid")),
            recording("d.mp3", "mp3", None, 240.0, None, Some("USABC1234567")),
        ];
        let groups = group_recordings(&recordings);
        assert_eq!(groups.len(), 3);
        assert!(groups.iter().any(|group| group.members.len() == 2));
    }

    #[test]
    fn title_similarity_is_not_evidence() {
        let recordings = vec![
            recording("a.mp3", "mp3", None, 180.0, Some("fp:first"), None),
            recording("b.mp3", "mp3", None, 180.0, Some("fp:second"), None),
        ];
        assert_eq!(group_recordings(&recordings).len(), 2);
    }

    #[test]
    fn equal_quality_uses_path_order() {
        let recordings = vec![
            recording("z.mp3", "mp3", Some(256), 180.0, Some("fp:same"), None),
            recording("a.mp3", "mp3", Some(256), 180.0, Some("fp:same"), None),
        ];
        let groups = group_recordings(&recordings);
        assert_eq!(groups[0].representative, 1);
    }

    #[tokio::test]
    async fn fallback_hash_cache_invalidates_when_the_file_changes() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("hash-cache.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let path = directory.path().join("audio.bin");
        tokio::fs::write(&path, b"first").await.unwrap();
        let first = sha256_cached(&pool, &path).await.unwrap();
        let reused = sha256_cached(&pool, &path).await.unwrap();
        assert_eq!(first, reused);

        tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        tokio::fs::write(&path, b"other").await.unwrap();
        let changed = sha256_cached(&pool, &path).await.unwrap();
        assert_ne!(first, changed);
    }
}
