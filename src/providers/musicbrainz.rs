use super::Candidate;
use anyhow::{Result, bail};
use reqwest::Client;
use serde_json::Value;

pub async fn recording(client: &Client, user_agent: &str, id: &str) -> Result<Candidate> {
    if user_agent.contains("configure-your-contact") {
        bail!("configure a meaningful MusicBrainz User-Agent");
    }
    let raw: Value = client
        .get(format!("https://musicbrainz.org/ws/2/recording/{id}"))
        .query(&[("fmt", "json"), ("inc", "artists+releases+isrcs")])
        .header("User-Agent", user_agent)
        .send()
        .await?
        .error_for_status()?
        .json()
        .await?;
    let release = raw["releases"].as_array().and_then(|v| v.first());
    let artist = raw["artist-credit"].as_array().and_then(|v| v.first());
    let artist_obj = artist.and_then(|v| v.get("artist"));
    let release_id = release.and_then(|v| v["id"].as_str()).map(str::to_owned);
    Ok(Candidate {
        title: raw["title"].as_str().unwrap_or("Unknown Title").into(),
        artist: artist
            .and_then(|v| v["name"].as_str())
            .unwrap_or("Unknown Artist")
            .into(),
        album: release.and_then(|v| v["title"].as_str()).map(str::to_owned),
        year: release.and_then(|v| v["date"].as_str()).map(str::to_owned),
        isrc: raw["isrcs"]
            .as_array()
            .and_then(|v| v.first())
            .and_then(Value::as_str)
            .map(str::to_owned),
        recording_id: Some(id.into()),
        release_id: release_id.clone(),
        artist_id: artist_obj.and_then(|v| v["id"].as_str()).map(str::to_owned),
        cover_url: release_id.map(|v| format!("https://coverartarchive.org/release/{v}/front-500")),
        raw_json: raw.to_string(),
        ..Default::default()
    })
}
