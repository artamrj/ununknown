use anyhow::{Result, bail};
use reqwest::Client;
use serde::Deserialize;

#[derive(Debug)]
pub struct Hit {
    pub score: f64,
    pub recording_id: String,
}
#[derive(Deserialize)]
struct Response {
    status: String,
    error: Option<ApiError>,
    #[serde(default)]
    results: Vec<ResultItem>,
}
#[derive(Deserialize)]
struct ApiError {
    message: String,
}
#[derive(Deserialize)]
struct ResultItem {
    score: f64,
    recordings: Option<Vec<Recording>>,
}
#[derive(Deserialize)]
struct Recording {
    id: String,
}

pub async fn lookup(
    client: &Client,
    key: &str,
    fingerprint: &str,
    duration: f64,
) -> Result<Vec<Hit>> {
    let response: Response = client
        .post("https://api.acoustid.org/v2/lookup")
        .form(&[
            ("client", key),
            ("meta", "recordings"),
            ("fingerprint", fingerprint),
            ("duration", &duration.round().to_string()),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if response.status != "ok" {
        bail!(
            "AcoustID rejected the request: {}",
            response
                .error
                .map(|error| error.message)
                .unwrap_or_else(|| "unknown API error".into())
        );
    }
    Ok(response
        .results
        .into_iter()
        .flat_map(|r| {
            r.recordings
                .unwrap_or_default()
                .into_iter()
                .map(move |v| Hit {
                    score: r.score,
                    recording_id: v.id,
                })
        })
        .collect())
}

#[allow(dead_code)]
pub async fn test_key(client: &Client, key: &str, fingerprint: &str, duration: f64) -> Result<()> {
    if key.trim().is_empty() {
        bail!("AcoustID API key is not configured");
    }
    let response: Response = client
        .post("https://api.acoustid.org/v2/lookup")
        .form(&[
            ("client", key),
            ("meta", "recordings"),
            ("fingerprint", fingerprint),
            ("duration", &duration.round().to_string()),
        ])
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    if response.status != "ok" {
        let message = response
            .error
            .map(|error| error.message)
            .unwrap_or_default();
        if message.to_ascii_lowercase().contains("invalid client") {
            bail!("AcoustID rejected the configured API key: {message}");
        }
    }
    Ok(())
}
