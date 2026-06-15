use anyhow::Result;
use reqwest::Client;
use serde::Deserialize;

#[derive(Debug)]
pub struct Hit {
    pub score: f64,
    pub recording_id: String,
}
#[derive(Deserialize)]
struct Response {
    results: Vec<ResultItem>,
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
        .get("https://api.acoustid.org/v2/lookup")
        .query(&[
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
