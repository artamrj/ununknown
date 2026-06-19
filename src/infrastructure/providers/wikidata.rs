use super::Candidate;
use crate::{
    domain::audio::AudioInfo,
    infrastructure::provider_cache::{ProviderCache, search_key},
};
use anyhow::Result;
use chrono::{Duration, Utc};
use reqwest::Client;
use serde_json::Value;
use sqlx::SqlitePool;

pub async fn search(
    pool: &SqlitePool,
    client: &Client,
    current: &AudioInfo,
) -> Result<Vec<Candidate>> {
    let Some(title) = current
        .title
        .as_deref()
        .filter(|value| !value.trim().is_empty())
    else {
        return Ok(Vec::new());
    };
    let artist = current.artist.as_deref().unwrap_or_default();
    let key = search_key(&format!("{artist} {title}"));
    let raw = if let Some(value) = ProviderCache::get(pool, "wikidata", &key).await? {
        value
    } else {
        let value: Value = client
            .get("https://www.wikidata.org/w/api.php")
            .query(&[
                ("action", "wbsearchentities"),
                ("format", "json"),
                ("language", "en"),
                ("limit", "5"),
                ("search", &format!("{artist} {title}")),
            ])
            .header("User-Agent", "Ununknown/0.6.0")
            .timeout(std::time::Duration::from_secs(4))
            .send()
            .await?
            .error_for_status()?
            .json()
            .await?;
        ProviderCache::put(
            pool,
            "wikidata",
            &key,
            &value,
            Utc::now() + Duration::days(30),
        )
        .await?;
        value
    };
    Ok(candidates_from_query(&raw))
}

fn candidates_from_query(raw: &Value) -> Vec<Candidate> {
    raw["search"]
        .as_array()
        .into_iter()
        .flatten()
        .take(5)
        .map(|row| Candidate {
            provider: "wikidata".into(),
            title: row["label"].as_str().unwrap_or("Unknown Title").into(),
            artist: row["description"]
                .as_str()
                .unwrap_or("Unknown Artist")
                .into(),
            release_id: row["id"].as_str().map(|value| format!("wikidata:{value}")),
            score_breakdown: Some(serde_json::json!({"source": "wikidata_search"}).to_string()),
            raw_json: row.to_string(),
            ..Default::default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wikidata_search() {
        let raw = serde_json::json!({
            "search": [{
                "id": "Q1",
                "label": "Song",
                "description": "song by Artist"
            }]
        });
        let candidates = candidates_from_query(&raw);
        assert_eq!(candidates[0].provider, "wikidata");
        assert_eq!(candidates[0].title, "Song");
        assert_eq!(candidates[0].release_id.as_deref(), Some("wikidata:Q1"));
    }
}
