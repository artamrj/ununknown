use anyhow::{Context, Result, bail};
use serde::Deserialize;
use std::{path::Path, time::Duration};
use tokio::process::Command;

#[derive(Deserialize)]
struct Output {
    fingerprint: String,
    duration: f64,
}

pub async fn calculate(path: &Path) -> Result<(String, f64)> {
    let result = tokio::time::timeout(
        Duration::from_secs(180),
        Command::new("fpcalc").arg("-json").arg(path).output(),
    )
    .await
    .context("fpcalc timed out")??;
    if !result.status.success() {
        bail!("fpcalc failed: {}", String::from_utf8_lossy(&result.stderr));
    }
    let parsed: Output = serde_json::from_slice(&result.stdout).context("invalid fpcalc output")?;
    Ok((parsed.fingerprint, parsed.duration))
}
