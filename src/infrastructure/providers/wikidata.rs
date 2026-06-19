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
    let query = sparql(title, artist);
    let key = search_key(&format!("{artist} {title}"));
    let raw = if let Some(value) = ProviderCache::get(pool, "wikidata", &key).await? {
        value
    } else {
        let value: Value = client
            .get("https://query.wikidata.org/sparql")
            .query(&[("format", "json"), ("query", query.as_str())])
            .header("User-Agent", "Ununknown/0.6.0")
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

fn sparql(title: &str, artist: &str) -> String {
    format!(
        r#"
SELECT ?work ?workLabel ?artistLabel WHERE {{
  ?work rdfs:label ?workLabel.
  FILTER(LANG(?workLabel) = "en")
  FILTER(CONTAINS(LCASE(STR(?workLabel)), LCASE("{}")))
  OPTIONAL {{ ?work wdt:P175 ?artist. ?artist rdfs:label ?artistLabel FILTER(LANG(?artistLabel) = "en") }}
  FILTER("{}" = "" || CONTAINS(LCASE(STR(?artistLabel)), LCASE("{}")))
  SERVICE wikibase:label {{ bd:serviceParam wikibase:language "en". }}
}}
LIMIT 5
"#,
        escape(title),
        escape(artist),
        escape(artist)
    )
}

fn escape(value: &str) -> String {
    value.replace('\\', "\\\\").replace('"', "\\\"")
}

fn candidates_from_query(raw: &Value) -> Vec<Candidate> {
    raw["results"]["bindings"]
        .as_array()
        .into_iter()
        .flatten()
        .take(5)
        .map(|row| Candidate {
            provider: "wikidata".into(),
            title: row["workLabel"]["value"]
                .as_str()
                .unwrap_or("Unknown Title")
                .into(),
            artist: row["artistLabel"]["value"]
                .as_str()
                .unwrap_or("Unknown Artist")
                .into(),
            release_id: row["work"]["value"].as_str().map(str::to_owned),
            score_breakdown: Some(serde_json::json!({"source": "wikidata_sparql"}).to_string()),
            raw_json: row.to_string(),
            ..Default::default()
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_wikidata_bindings() {
        let raw = serde_json::json!({
            "results": {"bindings": [{
                "work": {"value": "http://www.wikidata.org/entity/Q1"},
                "workLabel": {"value": "Song"},
                "artistLabel": {"value": "Artist"}
            }]}
        });
        let candidates = candidates_from_query(&raw);
        assert_eq!(candidates[0].provider, "wikidata");
        assert_eq!(candidates[0].title, "Song");
    }
}
