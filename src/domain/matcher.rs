use crate::domain::audio::AudioInfo;
use strsim::normalized_levenshtein;

pub fn score(
    acoustid_score: f64,
    current: &AudioInfo,
    title: &str,
    artist: &str,
    duration: f64,
) -> f64 {
    let mut score = acoustid_score * 70.0;
    if let Some(value) = &current.title {
        score += normalized_levenshtein(&value.to_lowercase(), &title.to_lowercase()) * 8.0;
    }
    if let Some(value) = &current.artist {
        score += normalized_levenshtein(&value.to_lowercase(), &artist.to_lowercase()) * 8.0;
    }
    let delta = (current.duration - duration).abs();
    score += if delta <= 2.0 {
        14.0
    } else if delta <= 5.0 {
        7.0
    } else {
        0.0
    };
    score.clamp(0.0, 100.0)
}

pub fn text_score(current: &AudioInfo, title: &str, artist: &str) -> f64 {
    let title_score = current
        .title
        .as_deref()
        .map(|value| normalized_levenshtein(&value.to_lowercase(), &title.to_lowercase()))
        .unwrap_or_default();
    let artist_score = current
        .artist
        .as_deref()
        .map(|value| normalized_levenshtein(&value.to_lowercase(), &artist.to_lowercase()))
        .unwrap_or_default();
    ((title_score * 45.0) + (artist_score * 25.0)).clamp(0.0, 70.0)
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn exact_match_scores_high() {
        let info = AudioInfo {
            title: Some("Song".into()),
            artist: Some("Artist".into()),
            duration: 180.0,
            ..Default::default()
        };
        assert!(score(1.0, &info, "Song", "Artist", 180.0) >= 99.0);
    }
}
