use crate::{domain::audio::AudioInfo, types::CompilationPreference};
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
    pub acoustid_score: f64,
    pub current: &'a AudioInfo,
    pub title: &'a str,
    pub artist: &'a str,
    pub album: Option<&'a str>,
    pub candidate_duration: Option<f64>,
    pub is_compilation: bool,
    pub compilation_preference: CompilationPreference,
}

pub fn score(input: CandidateInput<'_>) -> ScoreBreakdown {
    let duration_delta = input
        .candidate_duration
        .map(|duration| (input.current.duration - duration).abs());
    let duration = duration_delta.map(duration_match).unwrap_or(0.5);
    let title = input
        .current
        .title
        .as_deref()
        .map(|value| text_similarity(value, input.title))
        .unwrap_or_default();
    let artist = input
        .current
        .artist
        .as_deref()
        .map(|value| text_similarity(value, input.artist))
        .unwrap_or_default();
    let album_context = match (input.current.album.as_deref(), input.album) {
        (Some(current), Some(candidate)) => text_similarity(current, candidate),
        _ => 0.0,
    };
    let compilation_adjustment = match (input.is_compilation, input.compilation_preference) {
        (true, CompilationPreference::Avoid) => -0.08,
        (true, CompilationPreference::Prefer) => 0.05,
        (false, CompilationPreference::Prefer) => -0.03,
        _ => 0.0,
    };
    let final_score = ((0.45 * input.acoustid_score.clamp(0.0, 1.0))
        + (0.20 * duration)
        + (0.15 * title)
        + (0.10 * artist)
        + (0.10 * album_context)
        + compilation_adjustment)
        .clamp(0.0, 1.0)
        * 100.0;
    ScoreBreakdown {
        acoustid: input.acoustid_score.clamp(0.0, 1.0),
        duration,
        title,
        artist,
        album_context,
        compilation_adjustment,
        final_score,
    }
}

pub fn text_score(current: &AudioInfo, title: &str, artist: &str) -> f64 {
    let title_score = current
        .title
        .as_deref()
        .map(|value| text_similarity(value, title))
        .unwrap_or_default();
    let artist_score = current
        .artist
        .as_deref()
        .map(|value| text_similarity(value, artist))
        .unwrap_or_default();
    ((title_score * 60.0) + (artist_score * 30.0)).clamp(0.0, 90.0)
}

pub fn auto_selectable(
    top_score: f64,
    second_score: Option<f64>,
    duration_delta: Option<f64>,
) -> bool {
    top_score >= 90.0
        && duration_delta.is_some_and(|delta| delta <= 3.0)
        && second_score.is_none_or(|score| top_score - score >= 10.0)
}

fn text_similarity(left: &str, right: &str) -> f64 {
    normalized_levenshtein(&left.to_lowercase(), &right.to_lowercase())
}

fn duration_match(delta: f64) -> f64 {
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
        let score = score(CandidateInput {
            acoustid_score: 1.0,
            current: &info,
            title: "Song",
            artist: "Artist",
            album: Some("Album"),
            candidate_duration: Some(180.0),
            is_compilation: false,
            compilation_preference: CompilationPreference::Avoid,
        });
        assert!(score.final_score >= 99.0);
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
    fn compilation_preference_changes_score() {
        let info = AudioInfo {
            title: Some("Song".into()),
            artist: Some("Artist".into()),
            duration: 180.0,
            ..Default::default()
        };
        let avoid = score(CandidateInput {
            acoustid_score: 0.95,
            current: &info,
            title: "Song",
            artist: "Artist",
            album: None,
            candidate_duration: Some(180.0),
            is_compilation: true,
            compilation_preference: CompilationPreference::Avoid,
        });
        let prefer = score(CandidateInput {
            compilation_preference: CompilationPreference::Prefer,
            ..CandidateInput {
                acoustid_score: 0.95,
                current: &info,
                title: "Song",
                artist: "Artist",
                album: None,
                candidate_duration: Some(180.0),
                is_compilation: true,
                compilation_preference: CompilationPreference::Avoid,
            }
        });
        assert!(prefer.final_score > avoid.final_score);
    }
}
