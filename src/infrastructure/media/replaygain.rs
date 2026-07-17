use anyhow::{Context, Result, anyhow, bail};
use chrono::Utc;
use sqlx::SqlitePool;
use std::{path::Path, time::Duration};
use tokio::process::Command;

const ANALYSIS_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[derive(Clone, Copy, Debug, PartialEq)]
pub struct ReplayGain {
    pub track_gain_db: f64,
    pub track_peak: f64,
}

impl ReplayGain {
    pub fn gain_tag(self) -> String {
        format!("{:+.2} dB", self.track_gain_db)
    }

    pub fn peak_tag(self) -> String {
        format!("{:.6}", self.track_peak)
    }
}

pub fn available() -> bool {
    std::process::Command::new("ffmpeg")
        .args(["-hide_banner", "-filters"])
        .output()
        .is_ok_and(|output| {
            output.status.success()
                && String::from_utf8_lossy(&output.stdout)
                    .lines()
                    .any(|line| line.split_whitespace().any(|word| word == "replaygain"))
        })
}

/// Analyze decoded audio only. FFmpeg's null output means this never changes or
/// re-encodes the source file.
pub async fn get_or_analyze(pool: &SqlitePool, path: &Path) -> Result<ReplayGain> {
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

    let cached: Option<(f64, f64)> = sqlx::query_as(
        "SELECT track_gain_db,track_peak FROM replaygain_cache \
         WHERE path=? AND file_size=? AND file_mtime_ns=?",
    )
    .bind(path_text.as_ref())
    .bind(size)
    .bind(mtime_ns)
    .fetch_optional(pool)
    .await?;
    if let Some((track_gain_db, track_peak)) = cached {
        return validate(ReplayGain {
            track_gain_db,
            track_peak,
        });
    }

    let gain = analyze(path).await?;
    sqlx::query(
        "INSERT INTO replaygain_cache(path,file_size,file_mtime_ns,track_gain_db,track_peak,updated_at) \
         VALUES(?,?,?,?,?,?) ON CONFLICT(path) DO UPDATE SET \
         file_size=excluded.file_size,file_mtime_ns=excluded.file_mtime_ns,\
         track_gain_db=excluded.track_gain_db,track_peak=excluded.track_peak,updated_at=excluded.updated_at",
    )
    .bind(path_text.as_ref())
    .bind(size)
    .bind(mtime_ns)
    .bind(gain.track_gain_db)
    .bind(gain.track_peak)
    .bind(Utc::now().to_rfc3339())
    .execute(pool)
    .await?;
    Ok(gain)
}

async fn analyze(path: &Path) -> Result<ReplayGain> {
    let output = tokio::time::timeout(
        ANALYSIS_TIMEOUT,
        Command::new("ffmpeg")
            .arg("-hide_banner")
            .arg("-nostats")
            .arg("-nostdin")
            .arg("-i")
            .arg(path)
            .args(["-map", "0:a:0", "-af", "replaygain", "-f", "null", "-"])
            .output(),
    )
    .await
    .context("ReplayGain analysis timed out")?
    .context("could not start ffmpeg; install FFmpeg to add ReplayGain")?;
    if !output.status.success() {
        bail!(
            "ffmpeg ReplayGain analysis failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    parse(&String::from_utf8_lossy(&output.stderr))
}

fn parse(output: &str) -> Result<ReplayGain> {
    let mut track_gain_db = None;
    let mut track_peak = None;
    for line in output.lines() {
        if let Some(value) = line.split("track_gain =").nth(1) {
            track_gain_db = value
                .trim()
                .strip_suffix("dB")
                .map(str::trim)
                .and_then(|value| value.parse().ok());
        }
        if let Some(value) = line.split("track_peak =").nth(1) {
            track_peak = value.trim().parse().ok();
        }
    }
    validate(ReplayGain {
        track_gain_db: track_gain_db.ok_or_else(|| anyhow!("ffmpeg returned no track gain"))?,
        track_peak: track_peak.ok_or_else(|| anyhow!("ffmpeg returned no track peak"))?,
    })
}

fn validate(gain: ReplayGain) -> Result<ReplayGain> {
    if !gain.track_gain_db.is_finite() || !(-100.0..=100.0).contains(&gain.track_gain_db) {
        bail!("invalid ReplayGain track gain")
    }
    if !gain.track_peak.is_finite() || gain.track_peak < 0.0 || gain.track_peak > 100.0 {
        bail!("invalid ReplayGain track peak")
    }
    Ok(gain)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_ffmpeg_output_and_formats_standard_tags() {
        let value = parse(
            "[Parsed_replaygain_0] track_gain = -9.27 dB\n\
             [Parsed_replaygain_0] track_peak = 1.222379\n",
        )
        .unwrap();
        assert_eq!(value.gain_tag(), "-9.27 dB");
        assert_eq!(value.peak_tag(), "1.222379");
    }

    #[test]
    fn positive_gain_has_required_sign() {
        let value = ReplayGain {
            track_gain_db: 4.2,
            track_peak: 0.5,
        };
        assert_eq!(value.gain_tag(), "+4.20 dB");
    }

    #[test]
    fn rejects_missing_or_invalid_measurements() {
        assert!(parse("track_gain = -3.00 dB").is_err());
        assert!(parse("track_gain = NaN dB\ntrack_peak = 0.5").is_err());
        assert!(parse("track_gain = -3.00 dB\ntrack_peak = -0.1").is_err());
    }
}
