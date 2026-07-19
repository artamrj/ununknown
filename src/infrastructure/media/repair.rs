use crate::{domain::audio, infrastructure::media::integrity};
use anyhow::{Context, Result, bail};
use sqlx::SqlitePool;
use std::{
    path::{Path, PathBuf},
    time::{SystemTime, UNIX_EPOCH},
};
use tokio::process::Command;

const REPAIR_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(20 * 60);

#[derive(Clone, Debug)]
pub struct RepairResult {
    pub backup_path: PathBuf,
    pub original_duration: f64,
    pub repaired_duration: f64,
}

/// Salvages the decodable frames into a clean stream, validates the result, and
/// only then replaces the source. The damaged source is retained beside it with
/// an extension that the library scanner intentionally ignores.
pub async fn repair(pool: &SqlitePool, path: &Path) -> Result<RepairResult> {
    let original = tokio::task::spawn_blocking({
        let path = path.to_path_buf();
        move || audio::read(&path)
    })
    .await
    .context("audio metadata task failed")?
    .with_context(|| format!("could not read damaged source {}", path.display()))?;
    let extension = path
        .extension()
        .and_then(|value| value.to_str())
        .map(str::to_ascii_lowercase)
        .ok_or_else(|| anyhow::anyhow!("the damaged file has no supported extension"))?;
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let filename = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("audio");
    let temporary = path.with_file_name(format!(
        ".{filename}.ununknown-repair-{}-{stamp}.{extension}",
        std::process::id()
    ));
    let backup = unused_backup_path(path);

    let mut command = Command::new("ffmpeg");
    command.args([
        "-hide_banner",
        "-loglevel",
        "warning",
        "-nostdin",
        "-fflags",
        "+discardcorrupt",
        "-err_detect",
        "ignore_err",
        "-i",
    ]);
    command.arg(path).args([
        "-map",
        "0:a:0",
        "-vn",
        "-sn",
        "-dn",
        "-map_metadata",
        "0",
        "-map_chapters",
        "-1",
    ]);
    add_encoder(&mut command, &extension)?;
    command.arg("-y").arg(&temporary);

    let output = tokio::time::timeout(REPAIR_TIMEOUT, command.output())
        .await
        .context("audio repair timed out")?
        .context("could not start FFmpeg for audio repair")?;
    if !output.status.success() || !temporary.is_file() {
        let _ = tokio::fs::remove_file(&temporary).await;
        bail!(
            "FFmpeg could not salvage this audio stream: {}",
            bounded_diagnostic(&output.stderr)
        );
    }

    let repaired = match tokio::task::spawn_blocking({
        let temporary = temporary.clone();
        move || audio::read(&temporary)
    })
    .await
    {
        Ok(Ok(info)) => info,
        Ok(Err(error)) => {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(error)
                .context("FFmpeg produced a file whose audio metadata cannot be read");
        }
        Err(error) => {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(error).context("repaired audio metadata task failed");
        }
    };
    let minimum_duration = (original.duration * 0.5)
        .min(original.duration - 1.0)
        .max(1.0);
    if repaired.duration < minimum_duration {
        let _ = tokio::fs::remove_file(&temporary).await;
        bail!(
            "repair recovered only {:.1}s of {:.1}s; the source was left unchanged",
            repaired.duration,
            original.duration
        );
    }
    match integrity::check(pool, &temporary).await {
        Err(error) => {
            let _ = tokio::fs::remove_file(&temporary).await;
            return Err(error).context("could not validate the salvaged audio");
        }
        Ok(integrity::Integrity::Healthy) => {}
        Ok(integrity::Integrity::Corrupt(diagnostic)) => {
            let _ = tokio::fs::remove_file(&temporary).await;
            bail!("the salvaged copy still contains invalid audio: {diagnostic}");
        }
    }

    if let Err(error) = tokio::fs::rename(path, &backup).await {
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error)
            .with_context(|| format!("could not preserve damaged source as {}", backup.display()));
    }
    if let Err(error) = tokio::fs::rename(&temporary, path).await {
        let _ = tokio::fs::rename(&backup, path).await;
        let _ = tokio::fs::remove_file(&temporary).await;
        return Err(error).context("could not install repaired audio; the original was restored");
    }
    if let Err(error) = sqlx::query("DELETE FROM integrity_cache WHERE path IN (?,?)")
        .bind(path.to_string_lossy().as_ref())
        .bind(temporary.to_string_lossy().as_ref())
        .execute(pool)
        .await
    {
        tracing::warn!(path=%path.display(), "repaired audio but could not clear integrity cache: {error:#}");
    }

    Ok(RepairResult {
        backup_path: backup,
        original_duration: original.duration,
        repaired_duration: repaired.duration,
    })
}

fn add_encoder(command: &mut Command, extension: &str) -> Result<()> {
    match extension {
        "mp3" => {
            command.args(["-c:a", "libmp3lame", "-q:a", "2"]);
        }
        "flac" => {
            command.args(["-c:a", "flac"]);
        }
        "m4a" | "mp4" | "aac" => {
            command.args(["-c:a", "aac", "-b:a", "256k"]);
        }
        "ogg" => {
            command.args(["-c:a", "libvorbis", "-q:a", "6"]);
        }
        "opus" => {
            command.args(["-c:a", "libopus", "-b:a", "192k"]);
        }
        "wav" => {
            command.args(["-c:a", "pcm_s24le"]);
        }
        "aiff" | "aif" => {
            command.args(["-c:a", "pcm_s24be"]);
        }
        _ => bail!("automatic repair does not support .{extension} files"),
    }
    Ok(())
}

fn unused_backup_path(path: &Path) -> PathBuf {
    let filename = path
        .file_name()
        .and_then(|value| value.to_str())
        .unwrap_or("audio");
    let first = path.with_file_name(format!("{filename}.ununknown-damaged"));
    if !first.exists() {
        return first;
    }
    for suffix in 2..10_000 {
        let candidate = path.with_file_name(format!("{filename}.ununknown-damaged-{suffix}"));
        if !candidate.exists() {
            return candidate;
        }
    }
    path.with_file_name(format!(
        "{filename}.ununknown-damaged-{}",
        std::process::id()
    ))
}

fn bounded_diagnostic(stderr: &[u8]) -> String {
    String::from_utf8_lossy(stderr)
        .trim()
        .chars()
        .take(2_000)
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn mp3_and_flac_are_reencoded_and_backed_up() {
        if !integrity::available() {
            return;
        }
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("repair.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        for extension in ["mp3", "flac"] {
            let source = directory.path().join(format!("damaged.{extension}"));
            let status = std::process::Command::new("ffmpeg")
                .args(["-hide_banner", "-loglevel", "error", "-f", "lavfi", "-i"])
                .arg("sine=frequency=440:duration=4")
                .arg(&source)
                .status()
                .unwrap();
            assert!(status.success());

            let result = repair(&pool, &source).await.unwrap();

            assert!(source.is_file());
            assert!(result.backup_path.is_file());
            assert!(result.repaired_duration >= result.original_duration * 0.5);
            assert_eq!(
                integrity::check(&pool, &source).await.unwrap(),
                integrity::Integrity::Healthy
            );
        }
    }
}
