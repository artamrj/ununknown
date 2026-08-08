use super::*;

use crate::application::scan::persist::provider_display_name;

pub(crate) fn score_text_candidate(
    candidate: &mut infra_providers::Candidate,
    current: &audio::AudioInfo,
    source: &str,
) -> Result<()> {
    normalize_candidate_credits(candidate);
    preserve_album_context_for_catalog_single(candidate, current);
    let outcome = crate::domain::matcher::score_candidate(
        current,
        crate::domain::matcher::CandidateInput {
            title: &candidate.title,
            artist: &candidate.artist,
            album: candidate.album.as_deref(),
            candidate_duration: candidate.duration_delta,
        },
        crate::domain::matcher::ScoreMode::Text {
            source,
            provider_label: provider_display_name(&candidate.provider),
        },
        &candidate.provider,
    )?;
    candidate.duration_delta = outcome.duration_delta;
    candidate.score = outcome.score;
    candidate.score_breakdown = Some(outcome.breakdown_json);
    Ok(())
}
