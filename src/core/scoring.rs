use crate::{
    core::identify::{normalize_candidate_credits, preserve_album_context_for_catalog_single},
    domain::{
        audio::AudioInfo,
        matcher::{CandidateInput, ScoreMode, score_candidate},
    },
    providers::Candidate,
    workers::persist::provider_display_name,
};
use anyhow::Result;

pub(crate) fn score_text_candidate(
    candidate: &mut Candidate,
    current: &AudioInfo,
    source: &str,
) -> Result<()> {
    normalize_candidate_credits(candidate);
    preserve_album_context_for_catalog_single(candidate, current);
    let outcome = score_candidate(
        current,
        CandidateInput {
            title: &candidate.title,
            artist: &candidate.artist,
            album: candidate.album.as_deref(),
            candidate_duration: candidate.duration_delta,
        },
        ScoreMode::Text {
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
