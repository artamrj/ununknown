use anyhow::{Context, Result};
use chrono::Utc;
use sqlx::SqlitePool;
use std::{path::Path, time::Duration};
use tokio::process::Command;

const CHECK_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const MAX_DIAGNOSTIC_CHARS: usize = 2_000;

#[derive(Clone, Debug, Eq, PartialEq)]
pub enum Integrity {
    Healthy,
    Corrupt(String),
}

pub fn available() -> bool {
    std::process::Command::new("ffmpeg")
        .arg("-version")
        .output()
        .is_ok_and(|output| output.status.success())
}

/// Fully decodes the first audio stream into FFmpeg's null sink. No source data
/// is changed and no decoded audio is stored.
pub async fn check(pool: &SqlitePool, path: &Path) -> Result<Integrity> {
    let metadata = tokio::fs::metadata(path).await?;
    let size = i64::try_from(metadata.len()).context("audio file is too large")?;
    let mtime_ns: i64 = metadata
        .modified()?
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos()
        .try_into()
        .context("audio modification time is out of range")?;
    let path_text = path.to_string_lossy();
    let cached: Option<(bool, Option<String>)> = sqlx::query_as(
        "SELECT is_healthy,diagnostic FROM integrity_cache \
         WHERE path=? AND file_size=? AND file_mtime_ns=?",
    )
    .bind(path_text.as_ref())
    .bind(size)
    .bind(mtime_ns)
    .fetch_optional(pool)
    .await?;
    if let Some((is_healthy, diagnostic)) = cached
        && (is_healthy
            || !diagnostic
                .as_deref()
                .is_some_and(|detail| detail.contains("Invalid PNG signature")))
    {
        return Ok(if is_healthy {
            Integrity::Healthy
        } else {
            Integrity::Corrupt(diagnostic.unwrap_or_else(|| "Audio decoding failed".into()))
        });
    }

    let output = tokio::time::timeout(
        CHECK_TIMEOUT,
        Command::new("ffmpeg")
            .args([
                "-v",
                "error",
                "-xerror",
                "-err_detect",
                "explode",
                "-nostdin",
                "-i",
            ])
            .arg(path)
            .args(["-map", "0:a:0", "-f", "null", "-"])
            .output(),
    )
    .await
    .context("audio integrity check timed out")?
    .context("could not start ffmpeg; install FFmpeg to check audio integrity")?;
    let integrity = classify(
        output.status.success(),
        &String::from_utf8_lossy(&output.stderr),
    );
    let (is_healthy, diagnostic) = match &integrity {
        Integrity::Healthy => (true, None),
        Integrity::Corrupt(detail) => (false, Some(detail.as_str())),
    };
    sqlx::query(
        "INSERT INTO integrity_cache(path,file_size,file_mtime_ns,is_healthy,diagnostic,checked_at) \
         VALUES(?,?,?,?,?,?) ON CONFLICT(path) DO UPDATE SET \
         file_size=excluded.file_size,file_mtime_ns=excluded.file_mtime_ns,\
         is_healthy=excluded.is_healthy,diagnostic=excluded.diagnostic,checked_at=excluded.checked_at",
    )
    .bind(path_text.as_ref())
    .bind(size)
    .bind(mtime_ns)
    .bind(is_healthy)
    .bind(diagnostic)
    .bind(Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(integrity)
}

fn classify(success: bool, stderr: &str) -> Integrity {
    let diagnostic = stderr.trim();
    // FFmpeg may report a malformed attached picture while returning success
    // because the selected audio stream decoded completely. Artwork is replaced
    // during tag writing, so only a failed audio decode blocks the track.
    if success {
        return Integrity::Healthy;
    }
    let diagnostic = if diagnostic.is_empty() {
        "FFmpeg could not completely decode the audio stream".to_owned()
    } else {
        diagnostic.chars().take(MAX_DIAGNOSTIC_CHARS).collect()
    };
    Integrity::Corrupt(diagnostic)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{Seek, SeekFrom, Write};

    #[test]
    fn clean_decode_is_healthy() {
        assert_eq!(classify(true, ""), Integrity::Healthy);
    }

    #[test]
    fn non_audio_warning_does_not_mark_successful_decode_corrupt() {
        assert_eq!(
            classify(true, "Invalid PNG signature 0xFFD8FFE0"),
            Integrity::Healthy
        );
    }

    #[test]
    fn diagnostic_is_bounded() {
        let Integrity::Corrupt(detail) = classify(false, &"x".repeat(3_000)) else {
            panic!("expected corrupt result")
        };
        assert_eq!(detail.chars().count(), MAX_DIAGNOSTIC_CHARS);
    }

    #[tokio::test]
    async fn real_decode_accepts_healthy_audio_and_rejects_damaged_frames() {
        if !available() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let healthy = directory.path().join("healthy.mp3");
        let damaged = directory.path().join("damaged.mp3");
        let status = std::process::Command::new("ffmpeg")
            .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
            .arg("sine=frequency=440:duration=2")
            .args(["-q:a", "4"])
            .arg(&healthy)
            .status()
            .unwrap();
        assert!(status.success());
        std::fs::copy(&healthy, &damaged).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .write(true)
            .open(&damaged)
            .unwrap();
        file.seek(SeekFrom::Start(1_000)).unwrap();
        file.write_all(&[0; 1_000]).unwrap();
        drop(file);

        let database = directory.path().join("integrity.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        assert_eq!(check(&pool, &healthy).await.unwrap(), Integrity::Healthy);
        assert!(matches!(
            check(&pool, &damaged).await.unwrap(),
            Integrity::Corrupt(detail) if detail.contains("Invalid data") || detail.contains("Header missing")
        ));
        let cached: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM integrity_cache")
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(cached, 2);
    }
}
