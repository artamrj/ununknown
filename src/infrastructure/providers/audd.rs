use super::Candidate;
use crate::infrastructure::provider_cache::{ProviderCache, fingerprint_key};
use anyhow::{Context, Result, bail};
use chrono::{Duration, Utc};
use reqwest::{Client, multipart};
use serde_json::Value;
use sqlx::SqlitePool;
use std::path::Path;
use tokio::process::Command;

pub async fn recognize(
    pool: &SqlitePool,
    client: &Client,
    token: &str,
    path: &Path,
    fingerprint: &str,
    duration: f64,
) -> Result<Vec<Candidate>> {
    let cache_key = fingerprint_key(fingerprint);
    let raw = if let Some(value) = ProviderCache::get(pool, "audd", &cache_key).await? {
        value
    } else {
        let sample = audio_sample(path, duration).await?;
        let part = multipart::Part::bytes(sample)
            .file_name("recognition-sample.wav")
            .mime_str("audio/wav")?;
        let form = multipart::Form::new()
            .text("api_token", token.to_owned())
            .text("return", "musicbrainz,apple_music,spotify,deezer")
            .part("file", part);
        let value = client
            .post("https://api.audd.io/")
            .multipart(form)
            .send()
            .await?
            .error_for_status()?
            .json::<Value>()
            .await?;
        if value["status"] == "error" {
            bail!(
                "AudD API error: {}",
                value["error"]["error_message"]
                    .as_str()
                    .unwrap_or("unknown error")
            );
        }
        ProviderCache::put(
            pool,
            "audd",
            &cache_key,
            &value,
            Utc::now() + Duration::days(90),
        )
        .await?;
        value
    };
    Ok(parse_result(&raw).into_iter().collect())
}

async fn audio_sample(path: &Path, duration: f64) -> Result<Vec<u8>> {
    let start = if duration > 50.0 {
        (duration * 0.25).min(duration - 25.0)
    } else {
        0.0
    };
    let output = Command::new("ffmpeg")
        .args(["-v", "error", "-nostdin", "-ss"])
        .arg(format!("{start:.3}"))
        .arg("-i")
        .arg(path)
        .args([
            "-map", "0:a:0", "-t", "20", "-ac", "1", "-ar", "16000", "-f", "wav", "-",
        ])
        .output()
        .await
        .context("could not create AudD recognition sample with ffmpeg")?;
    if !output.status.success() || output.stdout.is_empty() {
        bail!(
            "could not create AudD sample: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        );
    }
    Ok(output.stdout)
}

fn parse_result(raw: &Value) -> Option<Candidate> {
    let result = raw.get("result")?;
    if result.is_null() {
        return None;
    }
    let spotify = &result["spotify"];
    let apple = &result["apple_music"];
    let title = result["title"]
        .as_str()
        .or_else(|| spotify["name"].as_str())?;
    let artist = result["artist"]
        .as_str()
        .or_else(|| spotify["artists"][0]["name"].as_str())?;
    let release_date = result["release_date"]
        .as_str()
        .or_else(|| spotify["album"]["release_date"].as_str())
        .map(str::to_owned);
    let cover_url = spotify["album"]["images"]
        .as_array()
        .and_then(|images| images.first())
        .and_then(|image| image["url"].as_str())
        .or_else(|| apple["artwork"]["url"].as_str())
        .map(|url| url.replace("{w}", "1200").replace("{h}", "1200"));
    let isrc = spotify["external_ids"]["isrc"]
        .as_str()
        .or_else(|| result["isrc"].as_str())
        .map(str::to_owned);
    let duration = spotify["duration_ms"].as_f64().map(|value| value / 1000.0);
    Some(Candidate {
        provider: "audd".into(),
        title: title.to_owned(),
        artist: artist.to_owned(),
        album: result["album"]
            .as_str()
            .or_else(|| spotify["album"]["name"].as_str())
            .map(str::to_owned),
        album_artist: spotify["album"]["artists"][0]["name"]
            .as_str()
            .map(str::to_owned),
        track_number: spotify["track_number"].as_i64(),
        disc_number: spotify["disc_number"].as_i64(),
        release_date: release_date.clone(),
        year: release_date
            .as_deref()
            .and_then(|date| date.get(..4))
            .map(str::to_owned),
        genre: apple["genreNames"][0].as_str().map(str::to_owned),
        label: result["label"].as_str().map(str::to_owned),
        isrc,
        cover_url,
        duration_delta: duration,
        score: 97.0,
        score_breakdown: Some(
            serde_json::json!({
                "audio_recognition": true,
                "source": "audd_audio_recognition",
                "sources": ["AudD"],
                "final_score": 97.0
            })
            .to_string(),
        ),
        raw_json: result.to_string(),
        ..Default::default()
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_recognition_and_nested_catalog_metadata() {
        let candidate = parse_result(&serde_json::json!({
            "result": {
                "artist": "Imagine Dragons", "title": "Warriors", "album": "Warriors",
                "release_date": "2014-09-18", "label": "Interscope",
                "spotify": {"duration_ms": 170066, "track_number": 18, "disc_number": 1,
                    "external_ids": {"isrc": "USUM71414163"},
                    "album": {"name": "Warriors", "release_date": "2014-09-18",
                        "artists": [{"name": "Imagine Dragons"}],
                        "images": [{"url": "https://example.test/cover.jpg"}]}}
            }
        }))
        .unwrap();
        assert_eq!(candidate.isrc.as_deref(), Some("USUM71414163"));
        assert_eq!(candidate.duration_delta, Some(170.066));
        assert_eq!(candidate.track_number, Some(18));
        assert!(
            candidate
                .score_breakdown
                .unwrap()
                .contains("audio_recognition")
        );
    }
}
