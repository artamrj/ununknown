use super::*;


pub(crate) fn score_text_candidate(
    candidate: &mut infra_providers::Candidate,
    current: &audio::AudioInfo,
    source: &str,
) -> Result<()> {
    normalize_candidate_credits(candidate);
    preserve_album_context_for_catalog_single(candidate, current);
    let title = current
        .title
        .as_deref()
        .map(|value| title_similarity(value, &candidate.title))
        .unwrap_or_default();
    let artist = current
        .artist
        .as_deref()
        .map(|value| artist_similarity(value, &candidate.artist))
        .unwrap_or_default();
    let album_context = match (current.album.as_deref(), candidate.album.as_deref()) {
        (Some(left), Some(right)) => text_similarity(left, right),
        _ => 0.0,
    };
    let candidate_duration = candidate.duration_delta;
    let duration_delta = candidate_duration.map(|value| (current.duration - value).abs());
    let duration = duration_delta.map(duration_match).unwrap_or(0.0);
    candidate.duration_delta = duration_delta;
    let provider_cap = match candidate.provider.as_str() {
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
        && candidate.album.is_some();
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
    candidate.score = score.min(provider_cap);
    let mut value = serde_json::json!({
        "acoustid": 0.0,
        "duration": duration,
        "title": title,
        "artist": artist,
        "album_context": album_context,
        "provider_text_only": true,
        "auto_select_rule": "Text-only matches require an exact unique title, artist, and duration or independent source agreement",
        "final_score": candidate.score
    });
    value["source"] = serde_json::Value::String(source.to_owned());
    value["sources"] = serde_json::json!([provider_display_name(&candidate.provider)]);
    candidate.score_breakdown = Some(value.to_string());
    Ok(())
}
