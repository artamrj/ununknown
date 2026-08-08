use crate::domain::audio::AudioInfo;
use serde::Serialize;
use strsim::normalized_levenshtein;

#[derive(Clone, Debug, Default, Serialize)]
pub struct ScoreBreakdown {
    pub acoustid: f64,
    pub duration: f64,
    pub title: f64,
    pub artist: f64,
    pub album_context: f64,
    pub compilation_adjustment: f64,
    pub final_score: f64,
}

#[derive(Clone, Debug)]
pub struct CandidateInput<'a> {
    pub title: &'a str,
    pub artist: &'a str,
    pub album: Option<&'a str>,
    pub candidate_duration: Option<f64>,
}

#[derive(Clone, Debug)]
pub enum ScoreMode<'a> {
    /// AcoustID-confirmed recording: weight by the acoustic fingerprint
    /// confidence, with a penalty for compilation releases.
    Acoustid {
        acoustid_score: f64,
        is_compilation: bool,
    },
    /// Text-only match: weight title/artist/album/duration similarity with a
    /// provider-specific ceiling and an explicit source label.
    Text {
        source: &'a str,
        provider_label: &'a str,
    },
}

pub struct ScoreOutcome {
    pub score: f64,
    pub duration_delta: Option<f64>,
    pub breakdown_json: String,
}

/// Single scoring entry point for the scan pipeline: acoustid-weighted and
/// text-only candidates share the same measurement primitives while keeping
/// their respective weighting, ceilings, and breakdown JSON shapes.
pub fn score_candidate(
    current: &AudioInfo,
    input: CandidateInput<'_>,
    mode: ScoreMode<'_>,
    provider: &str,
) -> Result<ScoreOutcome, serde_json::Error> {
    let duration_delta = input
        .candidate_duration
        .map(|duration| (current.duration - duration).abs());
    let (title, artist) = match &mode {
        ScoreMode::Acoustid { .. } => (
            current
                .title
                .as_deref()
                .map(|value| text_similarity(value, input.title))
                .unwrap_or_default(),
            current
                .artist
                .as_deref()
                .map(|value| text_similarity(value, input.artist))
                .unwrap_or_default(),
        ),
        ScoreMode::Text { .. } => (
            current
                .title
                .as_deref()
                .map(|value| title_similarity(value, input.title))
                .unwrap_or_default(),
            current
                .artist
                .as_deref()
                .map(|value| artist_similarity(value, input.artist))
                .unwrap_or_default(),
        ),
    };
    let album_context = match (current.album.as_deref(), input.album) {
        (Some(current), Some(candidate)) => text_similarity(current, candidate),
        _ => 0.0,
    };
    let duration = duration_delta.map(duration_match);
    match mode {
        ScoreMode::Acoustid {
            acoustid_score,
            is_compilation,
        } => {
            let duration = duration.unwrap_or(0.5);
            let compilation_adjustment = if is_compilation { -0.08 } else { 0.0 };
            let final_score = ((0.45 * acoustid_score.clamp(0.0, 1.0))
                + (0.20 * duration)
                + (0.15 * title)
                + (0.10 * artist)
                + (0.10 * album_context)
                + compilation_adjustment)
                .clamp(0.0, 1.0)
                * 100.0;
            let breakdown = ScoreBreakdown {
                acoustid: acoustid_score.clamp(0.0, 1.0),
                duration,
                title,
                artist,
                album_context,
                compilation_adjustment,
                final_score,
            };
            Ok(ScoreOutcome {
                score: final_score,
                duration_delta,
                breakdown_json: serde_json::to_string(&breakdown)?,
            })
        }
        ScoreMode::Text {
            source,
            provider_label,
        } => {
            let duration = duration.unwrap_or(0.0);
            let provider_cap = match provider {
                "itunes" => 94.0,
                "radiojavan" => 92.0,
                "audiomack" => 90.0,
                "navahang" => 92.0,
                "genius" => 90.0,
                "deezer" => 90.0,
                "musicbrainz" => 82.0,
                _ => 78.0,
            };
            let has_album_context = current
                .album
                .as_deref()
                .is_some_and(|album| !album.trim().is_empty() && !album.trim().starts_with('@'))
                && input.album.is_some();
            let score = if has_album_context {
                (0.35 * title) + (0.25 * artist) + (0.25 * album_context) + (0.15 * duration)
            } else if current
                .artist
                .as_deref()
                .is_some_and(|value| !value.trim().is_empty())
            {
                (0.45 * title) + (0.35 * artist) + (0.20 * duration)
            } else {
                (0.75 * title) + (0.25 * duration)
            }
            .clamp(0.0, 1.0)
                * 100.0;
            let score = score.min(provider_cap);
            let mut value = serde_json::json!({
                "acoustid": 0.0,
                "duration": duration,
                "title": title,
                "artist": artist,
                "album_context": album_context,
                "provider_text_only": true,
                "auto_select_rule": "Text-only matches require an exact unique title, artist, and duration or independent source agreement",
                "final_score": score
            });
            value["source"] = serde_json::Value::String(source.to_owned());
            value["sources"] = serde_json::json!([provider_label]);
            Ok(ScoreOutcome {
                score,
                duration_delta,
                breakdown_json: value.to_string(),
            })
        }
    }
}

#[cfg(test)]
pub fn auto_selectable(
    top_score: f64,
    second_score: Option<f64>,
    duration_delta: Option<f64>,
) -> bool {
    top_score >= 90.0
        && duration_delta.is_some_and(|delta| delta <= 3.0)
        && second_score.is_none_or(|score| top_score - score >= 10.0)
}

/// Canonical similarity and duration helpers shared by the scan pipeline,
/// approval, and completion layers. Keep these in one place so every decision
/// path measures strings and durations the same way.
pub fn text_similarity(left: &str, right: &str) -> f64 {
    normalized_levenshtein(&left.to_ascii_lowercase(), &right.to_ascii_lowercase())
}

pub fn title_similarity(left: &str, right: &str) -> f64 {
    let left_key = normalize_match_key(left);
    let right_key = normalize_match_key(right);
    let left_words = normalized_words(left);
    let right_words = normalized_words(right);
    let left_meaningful = meaningful_title_words(&left_words);
    let right_meaningful = meaningful_title_words(&right_words);
    let (shorter_words, longer_words) = if left_words.len() <= right_words.len() {
        (&left_words, &right_words)
    } else {
        (&right_words, &left_words)
    };
    if (!left_meaningful.is_empty() && left_meaningful == right_meaningful)
        || (left_key.len().min(right_key.len()) >= 4
            && (left_key.starts_with(&right_key) || right_key.starts_with(&left_key)))
        || (shorter_words.len() >= 3
            && shorter_words.iter().all(|word| longer_words.contains(word)))
    {
        0.96
    } else {
        text_similarity(left, right)
    }
}

pub fn artist_similarity(left: &str, right: &str) -> f64 {
    let direct = text_similarity(left, right);
    let left_key = normalize_match_key(left);
    let right_key = normalize_match_key(right);
    if left_key.len().min(right_key.len()) >= 5
        && (left_key.starts_with(&right_key) || right_key.starts_with(&left_key))
    {
        direct.max(0.92)
    } else {
        direct
    }
}

pub fn normalize_match_key(value: &str) -> String {
    value
        .chars()
        .filter(|ch| ch.is_alphanumeric())
        .flat_map(char::to_lowercase)
        .collect()
}

pub fn duration_match(delta: f64) -> f64 {
    if delta <= 3.0 {
        1.0
    } else if delta <= 8.0 {
        0.65
    } else if delta <= 15.0 {
        0.3
    } else {
        0.0
    }
}

pub fn text_close(left: &str, right: &str, threshold: f64) -> bool {
    if left.trim().is_empty() || right.trim().is_empty() {
        return false;
    }
    normalized_levenshtein(&left.to_ascii_lowercase(), &right.to_ascii_lowercase()) >= threshold
}

fn normalized_words(value: &str) -> Vec<String> {
    value
        .split(|ch: char| !ch.is_alphanumeric())
        .filter(|word| !word.is_empty())
        .map(str::to_lowercase)
        .collect()
}

fn meaningful_title_words(words: &[String]) -> Vec<&str> {
    words
        .iter()
        .map(String::as_str)
        .filter(|word| !matches!(*word, "a" | "an" | "the" | "such"))
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_match_scores_high() {
        let info = AudioInfo {
            title: Some("Song".into()),
            artist: Some("Artist".into()),
            album: Some("Album".into()),
            duration: 180.0,
            ..Default::default()
        };
        let outcome = score_candidate(
            &info,
            CandidateInput {
                title: "Song",
                artist: "Artist",
                album: Some("Album"),
                candidate_duration: Some(180.0),
            },
            ScoreMode::Acoustid {
                acoustid_score: 1.0,
                is_compilation: false,
            },
            "musicbrainz",
        )
        .unwrap();
        assert!(outcome.score >= 99.0);
    }

    #[test]
    fn bad_duration_blocks_auto_select() {
        assert!(!auto_selectable(96.0, None, Some(12.0)));
    }

    #[test]
    fn close_second_result_blocks_auto_select() {
        assert!(!auto_selectable(96.0, Some(91.0), Some(1.0)));
    }

    #[test]
    fn title_similarity_boosts_prefix_and_shared_word_matches() {
        assert!(title_similarity("Song", "Song") >= 0.95);
        assert!(title_similarity("The Song", "Song") >= 0.94);
        assert!(title_similarity("Song", "Completely Different") < 0.9);
    }

    #[test]
    fn artist_similarity_boosts_prefix_matches() {
        assert!(artist_similarity("Ed Sheeran", "Ed Sheeran") >= 0.95);
        assert!(artist_similarity("Ed Sheeran", "Ed Sheeran & Someone") >= 0.9);
    }

    #[test]
    fn duration_match_is_staircased() {
        assert_eq!(duration_match(2.0), 1.0);
        assert_eq!(duration_match(5.0), 0.65);
        assert_eq!(duration_match(12.0), 0.3);
        assert_eq!(duration_match(40.0), 0.0);
    }

    #[test]
    fn text_close_respects_threshold_and_blank() {
        assert!(text_close("hello world", "Hello World", 0.9));
        assert!(!text_close("", "hello", 0.5));
        assert!(!text_close("abc", "xyz", 0.5));
    }
}
