use crate::infrastructure::providers::{ArtworkCandidate, ArtworkStatus, Candidate};
use reqwest::Client;
use serde_json::Value;
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};

#[derive(Clone, Debug, Default, Eq, PartialEq)]
pub struct CompletionReport {
    pub score: u8,
    pub core_complete: bool,
    pub enriched_fields: Vec<String>,
    pub missing_fields: Vec<String>,
}

impl CompletionReport {
    pub fn summary(&self) -> String {
        if self.missing_fields.is_empty() {
            format!("metadata audit {}% complete", self.score)
        } else {
            format!(
                "metadata audit {}% complete; missing {}",
                self.score,
                self.missing_fields.join(", ")
            )
        }
    }
}

/// Complete a selected recording from corroborating catalog rows. Existing values
/// are never replaced, and release-specific fields are copied only from a matching
/// album so a compilation cannot silently change the chosen original release.
pub fn complete(
    selected: &mut Candidate,
    candidates: &[Candidate],
    existing_album: Option<&str>,
    embedded_cover: bool,
) -> CompletionReport {
    normalize_empty_fields(selected);
    let mut agreeing = candidates
        .iter()
        .filter(|candidate| same_recording(selected, candidate))
        .collect::<Vec<_>>();
    agreeing.sort_by(|left, right| donor_quality(right).total_cmp(&donor_quality(left)));

    let mut enriched = Vec::new();
    complete_release_identity(selected, &agreeing, &mut enriched);
    if selected.album.is_none()
        && let Some(donor) = choose_album_donor(&agreeing, existing_album)
    {
        fill_string(
            &mut selected.album,
            donor.album.as_ref(),
            "album",
            &mut enriched,
        );
        fill_string(
            &mut selected.album_artist,
            donor.album_artist.as_ref().or(Some(&donor.artist)),
            "album artist",
            &mut enriched,
        );
    }

    let release_donors = agreeing
        .iter()
        .copied()
        .filter(|candidate| {
            albums_compatible(selected.album.as_deref(), candidate.album.as_deref())
        })
        .collect::<Vec<_>>();
    if selected.album_artist.is_none() {
        let donor = release_donors
            .iter()
            .find_map(|candidate| candidate.album_artist.as_ref().or(Some(&candidate.artist)));
        fill_string(
            &mut selected.album_artist,
            donor,
            "album artist",
            &mut enriched,
        );
    }
    fill_from_release(
        &mut selected.track_number,
        &release_donors,
        |candidate| candidate.track_number,
        "track number",
        &mut enriched,
    );
    fill_from_release(
        &mut selected.track_total,
        &release_donors,
        |candidate| candidate.track_total,
        "track total",
        &mut enriched,
    );
    fill_from_release(
        &mut selected.disc_number,
        &release_donors,
        |candidate| candidate.disc_number,
        "disc number",
        &mut enriched,
    );
    fill_from_release(
        &mut selected.disc_total,
        &release_donors,
        |candidate| candidate.disc_total,
        "disc total",
        &mut enriched,
    );
    fill_release_string(
        &mut selected.release_date,
        &release_donors,
        |candidate| candidate.release_date.as_ref(),
        "release date",
        &mut enriched,
    );
    fill_release_string(
        &mut selected.year,
        &release_donors,
        |candidate| candidate.year.as_ref(),
        "year",
        &mut enriched,
    );
    if selected.year.is_none()
        && let Some(year) = selected
            .release_date
            .as_deref()
            .and_then(|date| date.get(..4))
    {
        selected.year = Some(year.to_owned());
        enriched.push("year".into());
    }
    fill_release_string(
        &mut selected.label,
        &release_donors,
        |candidate| candidate.label.as_ref(),
        "label",
        &mut enriched,
    );
    fill_release_string(
        &mut selected.release_country,
        &release_donors,
        |candidate| candidate.release_country.as_ref(),
        "release country",
        &mut enriched,
    );

    fill_any_string(
        &mut selected.genre,
        &agreeing,
        |candidate| candidate.genre.as_ref(),
        "genre",
        &mut enriched,
    );
    fill_any_string(
        &mut selected.composer,
        &agreeing,
        |candidate| candidate.composer.as_ref(),
        "composer",
        &mut enriched,
    );
    fill_any_string(
        &mut selected.isrc,
        &agreeing,
        |candidate| candidate.isrc.as_ref(),
        "ISRC",
        &mut enriched,
    );
    if selected.cover_url.is_none()
        && let Some(url) = agreeing
            .iter()
            .filter_map(|candidate| {
                candidate
                    .cover_url
                    .as_ref()
                    .map(|url| (artwork_priority(&candidate.provider), candidate.score, url))
            })
            .max_by(|left, right| {
                left.0
                    .cmp(&right.0)
                    .then_with(|| left.1.total_cmp(&right.1))
            })
            .map(|(_, _, url)| url)
    {
        selected.cover_url = Some(url.clone());
        enriched.push("cover".into());
    }

    let (album_defaulted, album_artist_defaulted) = normalize_release_fields(selected);
    if album_defaulted {
        enriched.push("album".into());
    }
    if album_artist_defaulted {
        enriched.push("album artist".into());
    }

    enriched.sort();
    enriched.dedup();
    let report = audit(selected, embedded_cover, enriched);
    record_report(selected, &agreeing, &report, embedded_cover);
    report
}

/// The recording identity a user can confirm independently of a catalog cover.
/// `normalize_release_fields` always supplies an album value, so this is
/// effectively title + artist. Used to let an explicit user choice become ready
/// even when no usable cover has been found yet.
pub fn identity_complete(candidate: &Candidate) -> bool {
    [
        nonempty(Some(&candidate.title)).is_some(),
        nonempty(Some(&candidate.artist)).is_some(),
        nonempty(candidate.album.as_deref()).is_some(),
    ]
    .into_iter()
    .all(|present| present)
}

pub fn audit(
    candidate: &Candidate,
    _embedded_cover: bool,
    enriched_fields: Vec<String>,
) -> CompletionReport {
    let cover = candidate.artwork_status == ArtworkStatus::Verified;
    let fields = [
        ("title", nonempty(Some(&candidate.title)).is_some(), 18_u8),
        ("artist", nonempty(Some(&candidate.artist)).is_some(), 18),
        ("album", nonempty(candidate.album.as_deref()).is_some(), 16),
        ("cover", cover, 16),
        (
            "year",
            nonempty(candidate.year.as_deref()).is_some()
                || nonempty(candidate.release_date.as_deref()).is_some(),
            10,
        ),
        ("genre", nonempty(candidate.genre.as_deref()).is_some(), 8),
        ("track number", candidate.track_number.is_some(), 6),
        (
            "album artist",
            nonempty(candidate.album_artist.as_deref()).is_some(),
            4,
        ),
        ("ISRC", nonempty(candidate.isrc.as_deref()).is_some(), 4),
    ];
    let score = fields
        .iter()
        .filter(|(_, present, _)| *present)
        .map(|(_, _, weight)| *weight)
        .sum();
    let missing_fields = fields
        .iter()
        .filter(|(_, present, _)| !*present)
        .map(|(name, _, _)| (*name).to_owned())
        .collect::<Vec<_>>();
    let core_complete = fields[..4].iter().all(|(_, present, _)| *present);
    CompletionReport {
        score,
        core_complete,
        enriched_fields,
        missing_fields,
    }
}

/// Download and validate the actual image that will be shown and embedded. If the
/// primary URL is broken, try the agreeing artwork alternatives collected during
/// catalog search before declaring the cover missing.
pub async fn ensure_usable_cover(
    pool: &SqlitePool,
    client: &Client,
    candidate: &mut Candidate,
    _embedded_cover: bool,
) -> bool {
    candidate.artwork_status = ArtworkStatus::Searching;
    candidate.artwork_message = Some("Checking matching catalog artwork".into());
    let mut artwork = candidate.artwork_candidates.clone();
    if let Some(url) = candidate.cover_url.as_deref()
        && !artwork.iter().any(|item| item.url == url)
    {
        artwork.push(ArtworkCandidate {
            provider: candidate.provider.clone(),
            url: url.to_owned(),
            release_id: candidate.release_id.clone(),
            isrc: candidate.isrc.clone(),
            album: candidate.album.clone(),
            artist: Some(candidate.artist.clone()),
            ..Default::default()
        });
    }
    if let Some(breakdown) = candidate
        .score_breakdown
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
    {
        for item in breakdown["artwork_candidates"]
            .as_array()
            .into_iter()
            .flatten()
        {
            if let Some(url) = item["url"].as_str()
                && !artwork.iter().any(|existing| existing.url == url)
            {
                artwork.push(ArtworkCandidate {
                    provider: item["provider"].as_str().unwrap_or("catalog").to_owned(),
                    url: url.to_owned(),
                    release_id: candidate.release_id.clone(),
                    isrc: candidate.isrc.clone(),
                    album: candidate.album.clone(),
                    artist: Some(candidate.artist.clone()),
                    ..Default::default()
                });
            }
        }
    }
    let (verified, mut saw_retryable, mut failures) =
        verify_artwork_candidates(pool, client, candidate, &artwork).await;
    if let Some((item, info)) = verified {
        accept_verified_artwork(candidate, artwork, &item, &info);
        return true;
    }
    if saw_retryable {
        mark_retryable_artwork(candidate, artwork, failures);
        return false;
    }
    let initial_count = artwork.len();

    // Existing release-backed URLs were unusable. Refresh only now, using the
    // selected recording identity, so healthy cached artwork stays instant.
    let (deezer_isrc, itunes, deezer) = tokio::join!(
        async {
            if let Some(isrc) = candidate.isrc.as_deref() {
                crate::infrastructure::providers::deezer::lookup_isrc(pool, client, isrc).await
            } else {
                Ok(None)
            }
        },
        crate::infrastructure::providers::itunes::search(
            pool,
            client,
            &candidate.title,
            Some(&candidate.artist),
            candidate.album.as_deref(),
        ),
        crate::infrastructure::providers::deezer::search(
            pool,
            client,
            &candidate.title,
            Some(&candidate.artist),
        )
    );
    match deezer_isrc {
        Ok(Some(source)) => {
            if let Some(url) = source.cover_url.clone()
                && !artwork.iter().any(|existing| existing.url == url)
            {
                artwork.push(ArtworkCandidate {
                    provider: source.provider,
                    url,
                    release_id: source.release_id,
                    isrc: source.isrc,
                    album: source.album,
                    artist: Some(source.artist),
                    ..Default::default()
                });
            }
        }
        Ok(None) => {}
        Err(error) => saw_retryable |= is_retryable_artwork_error(&error),
    }
    for result in [itunes, deezer] {
        match result {
            Ok(found) => {
                for source in found.into_iter().filter(|source| {
                    source.cover_url.is_some()
                        && same_recording(candidate, source)
                        && albums_compatible(candidate.album.as_deref(), source.album.as_deref())
                }) {
                    let url = source.cover_url.expect("filtered cover URL");
                    if artwork.iter().any(|existing| existing.url == url) {
                        continue;
                    }
                    artwork.push(ArtworkCandidate {
                        provider: source.provider,
                        url,
                        release_id: source.release_id,
                        isrc: source.isrc,
                        album: source.album,
                        artist: Some(source.artist),
                        ..Default::default()
                    });
                }
            }
            Err(error) => saw_retryable |= is_retryable_artwork_error(&error),
        }
    }
    let (verified, retryable, recovery_failures) =
        verify_artwork_candidates(pool, client, candidate, &artwork[initial_count..]).await;
    saw_retryable |= retryable;
    failures.extend(recovery_failures);
    if let Some((item, info)) = verified {
        accept_verified_artwork(candidate, artwork, &item, &info);
        return true;
    }
    if saw_retryable {
        mark_retryable_artwork(candidate, artwork, failures);
        return false;
    }
    candidate.cover_url = None;
    candidate.artwork_candidates = artwork;
    candidate.artwork_status = ArtworkStatus::CoverRequired;
    candidate.artwork_message = Some(if failures.is_empty() {
        "No matching catalog artwork was found; provide a cover URL".to_owned()
    } else {
        format!("No usable matching cover: {}", failures.join("; "))
    });
    mark_cover_verified(candidate, false, "cover_required");
    false
}

fn mark_retryable_artwork(
    candidate: &mut Candidate,
    artwork: Vec<ArtworkCandidate>,
    failures: Vec<String>,
) {
    if candidate.cover_url.is_none() {
        candidate.cover_url = artwork.first().map(|item| item.url.clone());
    }
    candidate.artwork_candidates = artwork;
    candidate.artwork_status = ArtworkStatus::RetryableError;
    candidate.artwork_message =
        Some("Cover download temporarily unavailable; retry will use the selected URL".into());
    record_artwork_failures(candidate, &failures);
    mark_cover_verified(candidate, false, "retryable_error");
}

fn record_artwork_failures(candidate: &mut Candidate, failures: &[String]) {
    let mut breakdown = candidate
        .score_breakdown
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    breakdown["metadata_completion"]["artwork_failures"] = serde_json::json!(failures);
    candidate.score_breakdown = Some(breakdown.to_string());
}

async fn verify_artwork_candidates(
    pool: &SqlitePool,
    client: &Client,
    candidate: &Candidate,
    artwork: &[ArtworkCandidate],
) -> (
    Option<(
        ArtworkCandidate,
        crate::infrastructure::media::tag_writer::ArtworkInfo,
    )>,
    bool,
    Vec<String>,
) {
    let mut saw_retryable = false;
    let mut failures = Vec::new();
    for item in artwork {
        if !artwork_matches_candidate(candidate, item) {
            failures.push(format!(
                "{} did not match the selected release",
                item.provider
            ));
            continue;
        }
        let trusted_exact = item.user_confirmed
            || item
                .release_id
                .as_deref()
                .zip(candidate.release_id.as_deref())
                .is_some_and(|(artwork_release, selected_release)| {
                    artwork_release == selected_release
                });
        match crate::infrastructure::providers::cover_art_archive::fetch_verified_url_cached(
            pool,
            client,
            &item.url,
            trusted_exact,
        )
        .await
        {
            Ok((_, info)) => {
                return (Some((item.clone(), info)), saw_retryable, failures);
            }
            Err(error) => {
                saw_retryable |= is_retryable_artwork_error(&error);
                failures.push(format!("{}: {error:#}", item.provider));
            }
        }
    }
    (None, saw_retryable, failures)
}

fn accept_verified_artwork(
    candidate: &mut Candidate,
    artwork: Vec<ArtworkCandidate>,
    selected: &ArtworkCandidate,
    info: &crate::infrastructure::media::tag_writer::ArtworkInfo,
) {
    candidate.cover_url = Some(selected.url.clone());
    candidate.artwork_candidates = artwork;
    candidate.artwork_status = ArtworkStatus::Verified;
    candidate.artwork_message = Some(format!(
        "Verified {}x{} {} cover from {}",
        info.width, info.height, info.mime, selected.provider
    ));
    mark_cover_verified(candidate, true, &selected.provider);
}

fn artwork_matches_candidate(candidate: &Candidate, artwork: &ArtworkCandidate) -> bool {
    if artwork.user_confirmed {
        return true;
    }
    if artwork
        .release_id
        .as_deref()
        .zip(candidate.release_id.as_deref())
        .is_some_and(|(left, right)| left == right)
    {
        return true;
    }
    if artwork
        .isrc
        .as_deref()
        .zip(candidate.isrc.as_deref())
        .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right))
    {
        let release_compatible = candidate
            .release_type
            .as_deref()
            .is_some_and(|kind| kind.eq_ignore_ascii_case("single"))
            || albums_compatible(artwork.album.as_deref(), candidate.album.as_deref());
        return release_compatible
            && artwork
                .artist
                .as_deref()
                .is_none_or(|artist| text_similarity(artist, &candidate.artist) >= 0.82);
    }
    artwork.album.as_deref().is_some_and(|album| {
        albums_compatible(Some(album), candidate.album.as_deref())
            && artwork
                .artist
                .as_deref()
                .is_some_and(|artist| text_similarity(artist, &candidate.artist) >= 0.82)
    })
}

fn is_retryable_artwork_error(error: &anyhow::Error) -> bool {
    error.chain().any(|source| {
        source
            .downcast_ref::<reqwest::Error>()
            .is_some_and(|error| {
                error.is_timeout()
                    || error.is_connect()
                    || error
                        .status()
                        .is_some_and(|status| status.as_u16() == 429 || status.is_server_error())
            })
    })
}

pub fn reassess(candidate: &mut Candidate, embedded_cover: bool) -> CompletionReport {
    normalize_release_fields(candidate);
    let enriched_fields = candidate
        .score_breakdown
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .and_then(|value| {
            value["metadata_completion"]["enriched_fields"]
                .as_array()
                .map(|fields| {
                    fields
                        .iter()
                        .filter_map(Value::as_str)
                        .map(str::to_owned)
                        .collect::<Vec<_>>()
                })
        })
        .unwrap_or_default();
    let report = audit(candidate, embedded_cover, enriched_fields);
    record_report(candidate, &[], &report, embedded_cover);
    report
}

/// Apply the canonical fallback tags used for standalone singles.
///
/// Returns which fields were changed so metadata-completion reports can include
/// the generated values.
pub fn normalize_release_fields(candidate: &mut Candidate) -> (bool, bool) {
    let original_album = candidate.album.clone();
    let confirmed_single = candidate
        .release_type
        .as_deref()
        .is_some_and(|kind| kind.eq_ignore_ascii_case("single"));
    if confirmed_single
        || candidate
            .album
            .as_deref()
            .is_none_or(|album| album.trim().is_empty())
    {
        candidate.album = Some("Single".to_owned());
    } else if let Some(album) = candidate.album.as_deref() {
        let cleaned = crate::domain::credits::release_title_without_featured(album);
        candidate.album = Some(if cleaned.is_empty() {
            "Single".to_owned()
        } else {
            cleaned
        });
    }
    let album_defaulted = candidate.album != original_album;

    let album_artist_defaulted = candidate
        .album_artist
        .as_deref()
        .is_none_or(|album_artist| album_artist.trim().is_empty());
    if album_artist_defaulted {
        if candidate.is_compilation {
            candidate.album_artist = Some("Various Artists".into());
            candidate.album_artist_credits = vec![crate::domain::credits::ArtistCredit::new(
                "Various Artists",
                "",
            )];
        } else {
            candidate.album_artist = Some(candidate.artist.clone());
            candidate.album_artist_credits = candidate.artist_credits.clone();
        }
    } else if candidate.album_artist_credits.is_empty()
        && let Some(album_artist) = candidate.album_artist.as_deref()
    {
        let credits = crate::domain::credits::normalize_featured(album_artist, "");
        candidate.album_artist = Some(credits.artist);
        candidate.album_artist_credits = credits.artists;
    }

    (album_defaulted, album_artist_defaulted)
}

fn complete_release_identity(
    selected: &mut Candidate,
    agreeing: &[&Candidate],
    enriched: &mut Vec<String>,
) {
    if selected.release_type.is_none()
        && let Some(kind) = agreeing
            .iter()
            .filter(|candidate| release_evidence_compatible(selected, candidate))
            .filter_map(|candidate| {
                inferred_release_type(candidate).map(|kind| {
                    (
                        provider_priority(&candidate.provider),
                        candidate.score as i64,
                        kind,
                    )
                })
            })
            .max_by_key(|(authority, score, _)| (*authority, *score))
            .map(|(_, _, kind)| kind)
    {
        selected.release_type = Some(kind);
        enriched.push("release type".into());
    }
}

fn release_evidence_compatible(selected: &Candidate, donor: &Candidate) -> bool {
    match (selected.album.as_deref(), donor.album.as_deref()) {
        (None, _) | (_, None) => true,
        (Some(selected), Some(donor)) => {
            let selected = comparable_release_title(selected);
            let donor = comparable_release_title(donor);
            selected == donor || strsim::normalized_levenshtein(&selected, &donor) >= 0.82
        }
    }
}

fn comparable_release_title(value: &str) -> String {
    let cleaned = crate::domain::credits::release_title_without_featured(value);
    let lower = cleaned.to_lowercase();
    let without_type = [" - single", " - ep"]
        .into_iter()
        .find_map(|suffix| lower.strip_suffix(suffix))
        .unwrap_or(&lower);
    normalized(without_type)
}

fn inferred_release_type(candidate: &Candidate) -> Option<String> {
    if let Some(kind) = candidate
        .release_type
        .as_deref()
        .map(str::trim)
        .filter(|kind| !kind.is_empty())
    {
        return Some(kind.to_ascii_lowercase());
    }
    let album = candidate.album.as_deref()?.trim().to_ascii_lowercase();
    if (album.ends_with(" - single") || album == "single")
        && candidate.track_total.is_none_or(|count| count == 1)
    {
        Some("single".into())
    } else if album.ends_with(" - ep") || album == "ep" {
        Some("ep".into())
    } else {
        None
    }
}

fn mark_cover_verified(candidate: &mut Candidate, verified: bool, source: &str) {
    let mut breakdown = candidate
        .score_breakdown
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    breakdown["metadata_completion"]["cover_verified"] = serde_json::json!(verified);
    breakdown["metadata_completion"]["cover_source"] = serde_json::json!(source);
    candidate.score_breakdown = Some(breakdown.to_string());
}

fn choose_album_donor<'a>(
    candidates: &[&'a Candidate],
    existing_album: Option<&str>,
) -> Option<&'a Candidate> {
    if let Some(existing) = existing_album.and_then(|value| nonempty(Some(value))) {
        let best = candidates
            .iter()
            .filter(|candidate| candidate.album.is_some())
            .map(|candidate| {
                (
                    text_similarity(existing, candidate.album.as_deref().unwrap_or_default()),
                    donor_quality(candidate),
                    *candidate,
                )
            })
            .max_by(|left, right| {
                left.0
                    .total_cmp(&right.0)
                    .then_with(|| left.1.total_cmp(&right.1))
            });
        if let Some((similarity, _, candidate)) = best
            && similarity >= 0.72
        {
            return Some(candidate);
        }
    }

    let mut frequency = HashMap::<String, usize>::new();
    for album in candidates
        .iter()
        .filter_map(|candidate| candidate.album.as_deref())
    {
        *frequency.entry(normalized(album)).or_default() += 1;
    }
    candidates
        .iter()
        .copied()
        .filter(|candidate| candidate.album.is_some())
        .max_by(|left, right| {
            album_donor_quality(left, &frequency).total_cmp(&album_donor_quality(right, &frequency))
        })
}

fn album_donor_quality(candidate: &Candidate, frequency: &HashMap<String, usize>) -> f64 {
    let agreement = candidate
        .album
        .as_deref()
        .map(normalized)
        .and_then(|album| frequency.get(&album).copied())
        .unwrap_or_default() as f64;
    donor_quality(candidate)
        + agreement * 8.0
        + if is_album_release(candidate) {
            8.0
        } else {
            0.0
        }
        - if candidate.is_compilation { 18.0 } else { 0.0 }
}

fn donor_quality(candidate: &Candidate) -> f64 {
    provider_priority(&candidate.provider) as f64 * 10.0
        + candidate.score.clamp(0.0, 100.0) * 0.08
        + [
            candidate.album.is_some(),
            candidate.cover_url.is_some(),
            candidate.year.is_some() || candidate.release_date.is_some(),
            candidate.genre.is_some(),
            candidate.track_number.is_some(),
        ]
        .into_iter()
        .filter(|present| *present)
        .count() as f64
}

fn same_recording(left: &Candidate, right: &Candidate) -> bool {
    if left
        .isrc
        .as_deref()
        .zip(right.isrc.as_deref())
        .is_some_and(|(left, right)| left.eq_ignore_ascii_case(right))
    {
        return true;
    }
    version_tags(&left.title) == version_tags(&right.title)
        && text_similarity(&left.title, &right.title) >= 0.90
        && text_similarity(&left.artist, &right.artist) >= 0.82
        && match (left.duration_delta, right.duration_delta) {
            (Some(left), Some(right)) => left <= 15.0 && right <= 15.0,
            _ => true,
        }
}

fn albums_compatible(selected: Option<&str>, donor: Option<&str>) -> bool {
    match (
        selected.and_then(|value| nonempty(Some(value))),
        donor.and_then(|value| nonempty(Some(value))),
    ) {
        (Some(selected), Some(donor)) => text_similarity(selected, donor) >= 0.72,
        _ => false,
    }
}

fn fill_string(
    target: &mut Option<String>,
    value: Option<&String>,
    name: &str,
    enriched: &mut Vec<String>,
) {
    if target.is_none()
        && let Some(value) = value.filter(|value| nonempty(Some(value)).is_some())
    {
        *target = Some(value.clone());
        enriched.push(name.to_owned());
    }
}

fn fill_from_release<T: Copy>(
    target: &mut Option<T>,
    donors: &[&Candidate],
    value: impl Fn(&Candidate) -> Option<T>,
    name: &str,
    enriched: &mut Vec<String>,
) {
    if target.is_none()
        && let Some(found) = donors.iter().find_map(|candidate| value(candidate))
    {
        *target = Some(found);
        enriched.push(name.to_owned());
    }
}

fn fill_release_string(
    target: &mut Option<String>,
    donors: &[&Candidate],
    value: impl Fn(&Candidate) -> Option<&String>,
    name: &str,
    enriched: &mut Vec<String>,
) {
    if target.is_none()
        && let Some(found) = donors.iter().find_map(|candidate| value(candidate))
    {
        *target = Some(found.clone());
        enriched.push(name.to_owned());
    }
}

fn fill_any_string(
    target: &mut Option<String>,
    donors: &[&Candidate],
    value: impl Fn(&Candidate) -> Option<&String>,
    name: &str,
    enriched: &mut Vec<String>,
) {
    fill_release_string(target, donors, value, name, enriched);
}

fn record_report(
    candidate: &mut Candidate,
    donors: &[&Candidate],
    report: &CompletionReport,
    embedded_cover: bool,
) {
    let mut breakdown = candidate
        .score_breakdown
        .as_deref()
        .and_then(|raw| serde_json::from_str::<Value>(raw).ok())
        .unwrap_or_else(|| serde_json::json!({}));
    let mut sources = breakdown["sources"]
        .as_array()
        .into_iter()
        .flatten()
        .filter_map(Value::as_str)
        .map(str::to_owned)
        .collect::<HashSet<_>>();
    sources.insert(provider_name(&candidate.provider).to_owned());
    for donor in donors {
        sources.insert(provider_name(&donor.provider).to_owned());
    }
    let mut sources = sources.into_iter().collect::<Vec<_>>();
    sources.sort();
    let cover_verified = breakdown["metadata_completion"]["cover_verified"]
        .as_bool()
        .unwrap_or(embedded_cover);
    let cover_source = breakdown["metadata_completion"]["cover_source"]
        .as_str()
        .map(str::to_owned)
        .unwrap_or_else(|| {
            if candidate.cover_url.is_some() {
                "catalog".into()
            } else if embedded_cover {
                "embedded".into()
            } else {
                "missing".into()
            }
        });
    let artwork_failures = breakdown["metadata_completion"]["artwork_failures"].clone();
    breakdown["sources"] = serde_json::json!(sources);
    breakdown["metadata_completion"] = serde_json::json!({
        "score": report.score,
        "core_complete": report.core_complete,
        "enriched_fields": report.enriched_fields,
        "missing_fields": report.missing_fields,
        "cover_source": cover_source,
        "cover_verified": cover_verified,
        "artwork_failures": artwork_failures
    });
    candidate.score_breakdown = Some(breakdown.to_string());
}

fn normalize_empty_fields(candidate: &mut Candidate) {
    for value in [
        &mut candidate.album,
        &mut candidate.album_artist,
        &mut candidate.year,
        &mut candidate.genre,
        &mut candidate.composer,
        &mut candidate.label,
        &mut candidate.isrc,
        &mut candidate.cover_url,
        &mut candidate.release_date,
        &mut candidate.release_country,
    ] {
        if value
            .as_deref()
            .is_some_and(|value| value.trim().is_empty())
        {
            *value = None;
        }
    }
}

fn provider_priority(provider: &str) -> u8 {
    match provider {
        "musicbrainz" | "itunes" | "spotify" | "radiojavan" | "audiomack" | "navahang"
        | "shazam" | "genius" => 6,
        "deezer" | "audd" | "songrec" => 5,
        "discogs" | "theaudiodb" => 4,
        "lastfm" | "soundcloud" => 3,
        _ => 2,
    }
}

fn artwork_priority(provider: &str) -> u8 {
    match provider {
        "itunes" | "spotify" | "shazam" => 8,
        "musicbrainz" | "radiojavan" | "audiomack" | "navahang" | "genius" => 7,
        "soundcloud" | "songrec" => 6,
        "deezer" => 5,
        "discogs" | "theaudiodb" => 4,
        _ => 2,
    }
}

fn is_album_release(candidate: &Candidate) -> bool {
    candidate
        .release_type
        .as_deref()
        .is_some_and(|kind| kind.eq_ignore_ascii_case("album"))
        && !candidate.is_compilation
}

fn version_tags(value: &str) -> HashSet<&'static str> {
    let normalized = normalized(value);
    let words = normalized.split_whitespace().collect::<HashSet<_>>();
    [
        ("live", &["live"] as &[&str]),
        ("remix", &["remix", "mix"] as &[&str]),
        ("acoustic", &["acoustic", "unplugged"] as &[&str]),
        ("instrumental", &["instrumental"] as &[&str]),
        ("karaoke", &["karaoke"] as &[&str]),
        ("demo", &["demo"] as &[&str]),
        ("stripped", &["stripped"] as &[&str]),
        ("sped", &["sped"] as &[&str]),
        ("slowed", &["slowed"] as &[&str]),
        ("duet", &["duet"] as &[&str]),
    ]
    .into_iter()
    .filter(|(_, aliases)| aliases.iter().any(|alias| words.contains(alias)))
    .map(|(tag, _)| tag)
    .collect()
}

fn text_similarity(left: &str, right: &str) -> f64 {
    strsim::normalized_levenshtein(&normalized(left), &normalized(right))
}

fn normalized(value: &str) -> String {
    value
        .chars()
        .map(|character| {
            if character.is_alphanumeric() {
                character.to_lowercase().collect::<String>()
            } else {
                " ".to_owned()
            }
        })
        .collect::<String>()
        .split_whitespace()
        .collect::<Vec<_>>()
        .join(" ")
}

fn nonempty(value: Option<&str>) -> Option<&str> {
    value.map(str::trim).filter(|value| !value.is_empty())
}

fn provider_name(provider: &str) -> &str {
    match provider {
        "itunes" => "Apple Music",
        "musicbrainz" => "MusicBrainz",
        "radiojavan" => "Radio Javan",
        "songrec" => "SongRec / Shazam",
        "audiomack" => "Audiomack",
        "navahang" => "Navahang",
        "shazam" => "Shazam",
        "genius" => "Genius",
        "theaudiodb" => "TheAudioDB",
        "lastfm" => "Last.fm",
        "soundcloud" => "SoundCloud",
        "spotify" => "Spotify",
        "deezer" => "Deezer",
        "audd" => "AudD",
        "discogs" => "Discogs",
        other => other,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn candidate(provider: &str, album: Option<&str>) -> Candidate {
        Candidate {
            provider: provider.into(),
            title: "Farangis".into(),
            artist: "Siavash Ghomayshi".into(),
            album: album.map(str::to_owned),
            duration_delta: Some(1.0),
            score: 90.0,
            ..Default::default()
        }
    }

    #[test]
    fn fills_missing_fields_from_same_recording_and_release() {
        let mut selected = candidate("musicbrainz", Some("Khabe Baroon"));
        let mut deezer = candidate("deezer", Some("Khabe Baroon"));
        deezer.album_artist = Some("Siavash Ghomayshi".into());
        deezer.year = Some("1993".into());
        deezer.genre = Some("Pop".into());
        deezer.track_number = Some(3);
        deezer.cover_url = Some("https://example.test/farangis.jpg".into());

        let report = complete(&mut selected, &[deezer], None, false);

        assert!(!report.core_complete);
        assert_eq!(selected.year.as_deref(), Some("1993"));
        assert_eq!(selected.genre.as_deref(), Some("Pop"));
        assert_eq!(selected.track_number, Some(3));
        assert_eq!(
            selected.cover_url.as_deref(),
            Some("https://example.test/farangis.jpg")
        );
    }

    #[test]
    fn does_not_merge_a_different_version_or_artist() {
        let mut selected = candidate("musicbrainz", Some("Khabe Baroon"));
        let mut remix = candidate("radiojavan", Some("Farangis Remix"));
        remix.title = "Farangis (Remix)".into();
        remix.cover_url = Some("https://example.test/remix.jpg".into());
        let mut other_artist = candidate("itunes", Some("Farangis - Single"));
        other_artist.artist = "Another Artist".into();
        other_artist.genre = Some("Rock".into());

        let report = complete(&mut selected, &[remix, other_artist], None, false);

        assert!(!report.core_complete);
        assert!(selected.cover_url.is_none());
        assert!(selected.genre.is_none());
    }

    #[test]
    fn embedded_artwork_does_not_bypass_catalog_verification() {
        let mut selected = candidate("musicbrainz", Some("Khabe Baroon"));
        let report = complete(&mut selected, &[], None, true);
        assert!(!report.core_complete);
        assert!(report.missing_fields.contains(&"cover".to_owned()));
    }

    #[test]
    fn defaults_missing_release_fields_for_a_single() {
        let mut selected = candidate("manual", None);

        let report = complete(&mut selected, &[], None, false);

        assert_eq!(selected.album.as_deref(), Some("Single"));
        assert_eq!(selected.album_artist.as_deref(), Some("Siavash Ghomayshi"));
        assert!(report.enriched_fields.contains(&"album".to_owned()));
        assert!(report.enriched_fields.contains(&"album artist".to_owned()));
    }

    #[test]
    fn normalizes_single_release_names_but_preserves_one_word_albums() {
        let mut single = candidate("itunes", Some("Farangis - Single"));
        single.release_type = Some("single".into());
        let mut album = candidate("musicbrainz", Some("Thriller"));

        normalize_release_fields(&mut single);
        normalize_release_fields(&mut album);

        assert_eq!(single.album.as_deref(), Some("Single"));
        assert_eq!(album.album.as_deref(), Some("Thriller"));
    }

    #[test]
    fn release_type_not_album_text_controls_single_normalization() {
        let mut named_single = candidate("musicbrainz", Some("Single Ladies"));
        named_single.release_type = Some("album".into());
        let mut credited_ep = candidate("musicbrainz", Some("Moorche (feat. Mehrad Hidden) - EP"));
        credited_ep.release_type = Some("ep".into());

        normalize_release_fields(&mut named_single);
        normalize_release_fields(&mut credited_ep);

        assert_eq!(named_single.album.as_deref(), Some("Single Ladies"));
        assert_eq!(credited_ep.album.as_deref(), Some("Moorche - EP"));
    }

    #[test]
    fn agreeing_catalog_release_type_turns_a_selected_deezer_release_into_single() {
        let mut selected = candidate("deezer", Some("Moorche (feat. Mehrad Hidden)"));
        selected.title = "Moorche".into();
        selected.artist = "Sepehr Khalse feat. Mehrad Hidden".into();
        let mut itunes = selected.clone();
        itunes.provider = "itunes".into();
        itunes.album = Some("Moorche (feat. Mehrad Hidden) - Single".into());
        itunes.release_type = Some("single".into());
        itunes.track_total = Some(1);

        complete(&mut selected, &[itunes], None, false);

        assert_eq!(selected.release_type.as_deref(), Some("single"));
        assert_eq!(selected.album.as_deref(), Some("Single"));
    }

    #[test]
    fn confirmed_single_accepts_its_isrc_matched_catalog_cover_title() {
        let mut selected = candidate("deezer", Some("Single"));
        selected.release_type = Some("single".into());
        selected.isrc = Some("QM-TEST-123".into());
        let artwork = ArtworkCandidate {
            provider: "Deezer".into(),
            url: "https://cdn-images.dzcdn.net/cover.jpg".into(),
            isrc: Some("qm-test-123".into()),
            album: Some("Moorche (feat. Mehrad Hidden)".into()),
            artist: Some("Siavash Ghomayshi".into()),
            ..Default::default()
        };

        assert!(artwork_matches_candidate(&selected, &artwork));
        selected.release_type = Some("album".into());
        assert!(!artwork_matches_candidate(&selected, &artwork));
    }

    #[tokio::test]
    async fn broken_primary_cover_falls_back_to_a_verified_catalog_image() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("cover-worker.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        for (url, data) in [
            ("https://example.test/broken.jpg", vec![0]),
            (
                "https://example.test/valid.png",
                crate::infrastructure::media::tag_writer::test_artwork_png(),
            ),
        ] {
            crate::infrastructure::provider_cache::ProviderCache::put(
                &pool,
                "artwork-url",
                &crate::infrastructure::provider_cache::search_key(url),
                &serde_json::json!({"data_base64": base64::Engine::encode(&base64::engine::general_purpose::STANDARD, data)}),
                chrono::Utc::now() + chrono::Duration::days(1),
            )
            .await
            .unwrap();
        }
        let mut selected = candidate("musicbrainz", Some("Khabe Baroon"));
        selected.cover_url = Some("https://example.test/broken.jpg".into());
        selected.score_breakdown = Some(
            serde_json::json!({
                "artwork_candidates": [
                    {"provider":"Deezer","url":"https://example.test/valid.png"}
                ]
            })
            .to_string(),
        );

        assert!(ensure_usable_cover(&pool, &reqwest::Client::new(), &mut selected, false).await);
        assert_eq!(
            selected.cover_url.as_deref(),
            Some("https://example.test/valid.png")
        );
    }

    #[tokio::test]
    async fn temporary_cover_failure_keeps_the_selected_url_for_retry() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("cover-retry.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let url = "http://127.0.0.1:1/temporarily-unavailable.jpg";
        let mut selected = candidate("manual", Some("Single"));
        selected.cover_url = Some(url.into());
        selected.artwork_candidates = vec![ArtworkCandidate {
            provider: "User verified".into(),
            url: url.into(),
            user_confirmed: true,
            ..Default::default()
        }];

        let usable = ensure_usable_cover(&pool, &Client::new(), &mut selected, false).await;

        assert!(!usable);
        assert_eq!(selected.cover_url.as_deref(), Some(url));
        assert_eq!(selected.artwork_status, ArtworkStatus::RetryableError);
        assert_eq!(
            selected.artwork_message.as_deref(),
            Some("Cover download temporarily unavailable; retry will use the selected URL")
        );
        assert!(!audit(&selected, false, Vec::new()).core_complete);
    }
}
