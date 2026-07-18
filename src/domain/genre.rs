use crate::{domain::audio::AudioInfo, infrastructure::providers::Candidate};
use anyhow::Result;
use serde::Serialize;
use serde_json::Value;
use std::collections::HashMap;

#[derive(Clone, Debug, Serialize)]
struct Evidence {
    source: String,
    supplied_value: String,
    normalized_genre: String,
    weight: f64,
}

#[derive(Debug, Serialize)]
struct Assessment {
    primary: String,
    alternatives: Vec<String>,
    confidence: f64,
    language: Option<String>,
    language_code: Option<String>,
    sources: Vec<Evidence>,
}

pub fn needs_artist_lookup(candidates: &[Candidate], current: &AudioInfo) -> bool {
    if soundtrack_context(candidates, current) {
        return false;
    }
    let specific = |raw: &str| {
        classify(raw).primary.is_some()
            && !matches!(
                raw.trim().to_lowercase().as_str(),
                "farsi" | "persian" | "worldwide" | "world"
            )
    };
    !candidates
        .iter()
        .filter(|candidate| relevant(candidate, current))
        .filter_map(|candidate| candidate.genre.as_deref())
        .any(specific)
}

pub fn enrich(
    candidates: &mut [Candidate],
    current: &AudioInfo,
    artist_genres: &[String],
) -> Result<()> {
    let mut evidence = Vec::new();
    let mut language = None;

    if let Some(raw) = current.genre.as_deref() {
        add_evidence(&mut evidence, &mut language, "Existing tag", raw, 0.68);
    }

    for candidate in candidates
        .iter()
        .filter(|candidate| relevant(candidate, current))
    {
        if let Some(raw) = candidate.genre.as_deref() {
            add_evidence(
                &mut evidence,
                &mut language,
                provider_name(&candidate.provider),
                raw,
                provider_weight(&candidate.provider),
            );
        }
        add_raw_provider_evidence(candidate, &mut evidence, &mut language);
    }

    for (index, raw) in artist_genres.iter().enumerate() {
        add_evidence(
            &mut evidence,
            &mut language,
            "Wikidata artist profile",
            raw,
            (0.72 - index as f64 * 0.03).max(0.60),
        );
    }

    if soundtrack_context(candidates, current) {
        add_evidence(
            &mut evidence,
            &mut language,
            "Track context",
            "Soundtrack",
            0.94,
        );
    }

    let Some(assessment) = assess(evidence, language) else {
        return Ok(());
    };
    for candidate in candidates {
        candidate.genre = Some(assessment.primary.clone());
        let mut breakdown = candidate
            .score_breakdown
            .as_deref()
            .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
            .unwrap_or_else(|| serde_json::json!({}));
        breakdown["genre"] = serde_json::to_value(&assessment)?;
        candidate.score_breakdown = Some(breakdown.to_string());
    }
    Ok(())
}

fn add_raw_provider_evidence(
    candidate: &Candidate,
    evidence: &mut Vec<Evidence>,
    language: &mut Option<(String, String, f64)>,
) {
    let Ok(raw) = serde_json::from_str::<Value>(&candidate.raw_json) else {
        return;
    };
    for key in ["styles", "genres"] {
        for value in raw[key]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
        {
            add_evidence(
                evidence,
                language,
                provider_name(&candidate.provider),
                value,
                provider_weight(&candidate.provider),
            );
        }
    }
    for tag in raw["tags"].as_array().into_iter().flatten() {
        if let Some(value) = tag["name"].as_str() {
            let count = tag["count"].as_f64().unwrap_or_default();
            add_evidence(
                evidence,
                language,
                provider_name(&candidate.provider),
                value,
                (0.64 + count.clamp(0.0, 4.0) * 0.03).min(0.76),
            );
        }
    }
}

fn add_evidence(
    evidence: &mut Vec<Evidence>,
    language: &mut Option<(String, String, f64)>,
    source: &str,
    raw: &str,
    weight: f64,
) {
    let classification = classify(raw);
    if let Some((name, code)) = classification.language
        && language.as_ref().is_none_or(|current| weight > current.2)
    {
        *language = Some((name.to_owned(), code.to_owned(), weight));
    }
    if let Some(primary) = classification.primary {
        evidence.push(Evidence {
            source: source.to_owned(),
            supplied_value: raw.to_owned(),
            normalized_genre: primary.to_owned(),
            weight: (weight * classification.quality).clamp(0.0, 1.0),
        });
    }
}

struct Classification {
    primary: Option<&'static str>,
    language: Option<(&'static str, &'static str)>,
    quality: f64,
}

fn classify(raw: &str) -> Classification {
    let value = raw
        .trim()
        .to_lowercase()
        .replace(['_', '-'], " ")
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ");
    let result = |primary, language, quality| Classification {
        primary,
        language,
        quality,
    };
    match value.as_str() {
        "farsi" | "persian" | "iranian" => {
            result(Some("Persian Pop"), Some(("Persian", "fas")), 0.70)
        }
        "persian pop" => result(Some("Persian Pop"), Some(("Persian", "fas")), 1.0),
        "persian rap" => result(Some("Hip-Hop/Rap"), Some(("Persian", "fas")), 1.0),
        "persian traditional"
        | "iranian traditional"
        | "iranian classical music"
        | "persian classical music" => {
            result(Some("Persian Traditional"), Some(("Persian", "fas")), 1.0)
        }
        "worldwide" | "world" | "world music" => result(Some("World"), None, 0.45),
        "rap" | "hip hop" | "hiphop" | "hip hop/rap" | "pop rap" | "trap" => {
            result(Some("Hip-Hop/Rap"), None, 1.0)
        }
        "r&b" | "rnb" | "r&b/soul" | "soul" | "alternative r&b" | "contemporary r&b" => {
            result(Some("R&B/Soul"), None, 1.0)
        }
        "alternative" | "indie" | "indie pop" | "dream pop" | "shoegaze" => {
            result(Some("Alternative"), None, 1.0)
        }
        "alternative rock" | "indie rock" | "rock" | "rock music" | "pop rock" | "punk"
        | "punk rock" | "post punk" | "hard rock" => result(Some("Rock"), None, 1.0),
        "pop" | "pop music" | "dance pop" | "electropop" | "synthpop" | "synth pop"
        | "adult contemporary" | "traditional pop" => result(Some("Pop"), None, 1.0),
        "soundtrack" | "stage & screen" | "film score" | "score" | "trailer music" => {
            result(Some("Soundtrack"), None, 1.0)
        }
        "electronic" | "electronica" | "dance" | "edm" | "house" | "deep house" | "techno"
        | "trance" | "ambient" | "downtempo" | "dubstep" => result(Some("Electronic"), None, 1.0),
        "folk" | "folk pop" | "traditional folk" | "contemporary folk" => {
            result(Some("Folk"), None, 1.0)
        }
        "jazz" => result(Some("Jazz"), None, 1.0),
        "classical" => result(Some("Classical"), None, 1.0),
        "country" | "country music" => result(Some("Country"), None, 1.0),
        "reggae" => result(Some("Reggae"), None, 1.0),
        "metal" | "heavy metal" | "alternative metal" | "nu metal" | "metalcore" => {
            result(Some("Metal"), None, 1.0)
        }
        "blues" => result(Some("Blues"), None, 1.0),
        "latin" | "latin pop" | "reggaeton" | "salsa" | "bachata" => {
            result(Some("Latin"), None, 1.0)
        }
        "turkish pop" => result(Some("Turkish Pop"), Some(("Turkish", "tur")), 1.0),
        "arabic pop" => result(Some("Arabic Pop"), Some(("Arabic", "ara")), 1.0),
        "k pop" | "kpop" => result(Some("K-Pop"), Some(("Korean", "kor")), 1.0),
        "children's music" | "childrens music" | "children" => {
            result(Some("Children's Music"), None, 1.0)
        }
        "singer/songwriter" | "singer songwriter" => result(Some("Singer/Songwriter"), None, 1.0),
        _ => result(None, None, 0.0),
    }
}

fn assess(evidence: Vec<Evidence>, language: Option<(String, String, f64)>) -> Option<Assessment> {
    let mut strongest_by_source = HashMap::<(String, String), Evidence>::new();
    for item in evidence {
        let key = (item.source.clone(), item.normalized_genre.clone());
        if strongest_by_source
            .get(&key)
            .is_none_or(|current| item.weight > current.weight)
        {
            strongest_by_source.insert(key, item);
        }
    }
    let sources = strongest_by_source.into_values().collect::<Vec<_>>();
    let mut scores = HashMap::<String, (f64, f64, usize)>::new();
    for item in &sources {
        let score = scores.entry(item.normalized_genre.clone()).or_default();
        score.0 += item.weight;
        score.1 = score.1.max(item.weight);
        score.2 += 1;
    }
    let mut ranked = scores.into_iter().collect::<Vec<_>>();
    ranked.sort_by(|left, right| right.1.0.total_cmp(&left.1.0));
    let (primary, (_, strongest, source_count)) = ranked.first()?.clone();
    let confidence = (strongest + 0.08 * source_count.saturating_sub(1) as f64).min(0.98);
    let alternatives = ranked
        .iter()
        .skip(1)
        .take(3)
        .map(|(genre, _)| genre.clone())
        .collect();
    let (language, language_code) = language
        .map(|(name, code, _)| (Some(name), Some(code)))
        .unwrap_or_default();
    Some(Assessment {
        primary,
        alternatives,
        confidence,
        language,
        language_code,
        sources,
    })
}

fn relevant(candidate: &Candidate, current: &AudioInfo) -> bool {
    let title = current.title.as_deref().unwrap_or_default();
    let artist = current.artist.as_deref().unwrap_or_default();
    similar(title, &candidate.title, 0.72) && similar(artist, &candidate.artist, 0.62)
}

fn soundtrack_context(candidates: &[Candidate], current: &AudioInfo) -> bool {
    let mut values = vec![
        current.title.as_deref().unwrap_or_default(),
        current.album.as_deref().unwrap_or_default(),
    ];
    for candidate in candidates
        .iter()
        .filter(|candidate| relevant(candidate, current))
    {
        values.push(&candidate.title);
        values.push(candidate.album.as_deref().unwrap_or_default());
        values.push(candidate.release_type.as_deref().unwrap_or_default());
    }
    values.iter().any(|value| {
        let value = value.to_lowercase();
        [
            "soundtrack",
            "motion picture",
            "trailer",
            "titraj",
            "film score",
        ]
        .iter()
        .any(|needle| value.contains(needle))
    })
}

fn similar(left: &str, right: &str, threshold: f64) -> bool {
    if left.trim().is_empty() || right.trim().is_empty() {
        return false;
    }
    let left = normalize(left);
    let right = normalize(right);
    left.starts_with(&right)
        || right.starts_with(&left)
        || strsim::normalized_levenshtein(&left, &right) >= threshold
}

fn normalize(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

fn provider_weight(provider: &str) -> f64 {
    match provider {
        "discogs" => 0.88,
        "itunes" => 0.84,
        "musicbrainz" => 0.78,
        "theaudiodb" => 0.76,
        "lastfm" => 0.72,
        "radiojavan" => 0.70,
        "audiomack" => 0.70,
        "genius" => 0.68,
        "soundcloud" => 0.68,
        _ => 0.64,
    }
}

fn provider_name(provider: &str) -> &str {
    match provider {
        "itunes" => "Apple Music",
        "musicbrainz" => "MusicBrainz",
        "discogs" => "Discogs",
        "theaudiodb" => "TheAudioDB",
        "lastfm" => "Last.fm",
        "radiojavan" => "Radio Javan",
        "audiomack" => "Audiomack",
        "genius" => "Genius",
        "soundcloud" => "SoundCloud",
        _ => provider,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(provider: &str, genre: &str) -> Candidate {
        Candidate {
            provider: provider.into(),
            title: "Song".into(),
            artist: "Artist".into(),
            genre: Some(genre.into()),
            raw_json: "{}".into(),
            ..Default::default()
        }
    }

    #[test]
    fn existing_specific_genre_beats_broad_language_category() {
        let current = AudioInfo {
            title: Some("Song".into()),
            artist: Some("Artist".into()),
            genre: Some("R&B".into()),
            ..Default::default()
        };
        let mut candidates = vec![candidate("itunes", "Farsi")];
        enrich(&mut candidates, &current, &[]).unwrap();
        assert_eq!(candidates[0].genre.as_deref(), Some("R&B/Soul"));
        let breakdown: Value =
            serde_json::from_str(candidates[0].score_breakdown.as_deref().unwrap()).unwrap();
        assert_eq!(breakdown["genre"]["language"], "Persian");
    }

    #[test]
    fn soundtrack_context_overrides_generic_catalog_category() {
        let current = AudioInfo {
            title: Some("The Day Has Come (Trailer)".into()),
            artist: Some("X-Ray Dog".into()),
            ..Default::default()
        };
        let mut candidates = vec![Candidate {
            provider: "itunes".into(),
            title: "The Day Has Come (Trailer)".into(),
            artist: "X-Ray Dog".into(),
            album: Some("Trailer - Single".into()),
            genre: Some("Worldwide".into()),
            raw_json: "{}".into(),
            ..Default::default()
        }];
        enrich(&mut candidates, &current, &[]).unwrap();
        assert_eq!(candidates[0].genre.as_deref(), Some("Soundtrack"));
    }
}
