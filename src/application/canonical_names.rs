use crate::{
    domain::credits::{clean_text, display_artist, identity_key},
    infrastructure::providers::{ArtistCredit, Candidate},
};
use anyhow::Result;
use chrono::Utc;
use sqlx::SqlitePool;
use std::collections::{HashMap, HashSet};

#[derive(Clone)]
struct Canonical {
    value: String,
    authority: i64,
}

pub async fn canonicalize_candidates(
    pool: &SqlitePool,
    candidates: &mut [Candidate],
) -> Result<()> {
    for candidate in candidates.iter_mut() {
        normalize_candidate_credits(candidate);
    }
    let mut winners = HashMap::<(String, String), Canonical>::new();
    for candidate in candidates.iter() {
        let authority = provider_authority(&candidate.provider);
        collect_credits(&mut winners, "artist", &candidate.artist_credits, authority);
        collect_credits(
            &mut winners,
            "artist",
            &candidate.album_artist_credits,
            authority,
        );
        winners
            .entry(("title".into(), title_identity(candidate)))
            .and_modify(|current| {
                if authority > current.authority {
                    *current = Canonical {
                        value: candidate.title.clone(),
                        authority,
                    };
                }
            })
            .or_insert_with(|| Canonical {
                value: candidate.title.clone(),
                authority,
            });
    }

    for ((kind, key), winner) in &mut winners {
        if let Some((value, authority)) = sqlx::query_as::<_, (String, i64)>(
            "SELECT canonical_value,authority FROM canonical_names WHERE kind=? AND identity_key=?",
        )
        .bind(kind)
        .bind(key)
        .fetch_optional(pool)
        .await?
            && authority >= winner.authority
        {
            winner.value = value;
            winner.authority = authority;
        }
        sqlx::query(
            "INSERT INTO canonical_names(kind,identity_key,canonical_value,authority,updated_at)
             VALUES(?,?,?,?,?) ON CONFLICT(kind,identity_key) DO UPDATE SET
             canonical_value=CASE WHEN excluded.authority>authority THEN excluded.canonical_value ELSE canonical_value END,
             authority=MAX(authority,excluded.authority),updated_at=excluded.updated_at",
        )
        .bind(kind)
        .bind(key)
        .bind(&winner.value)
        .bind(winner.authority)
        .bind(Utc::now().to_rfc3339())
        .execute(pool)
        .await?;
    }

    let known_names = collect_known_names(&winners, pool).await?;
    for candidate in &mut *candidates {
        resolve_ampersand_credits(&mut candidate.artist_credits, &known_names);
        resolve_ampersand_credits(&mut candidate.album_artist_credits, &known_names);
    }

    for candidate in candidates {
        apply_credits(&mut candidate.artist_credits, &winners);
        apply_credits(&mut candidate.album_artist_credits, &winners);
        if !candidate.artist_credits.is_empty() {
            candidate.artist = display_artist(&candidate.artist_credits);
        }
        if !candidate.album_artist_credits.is_empty() {
            candidate.album_artist = Some(display_artist(&candidate.album_artist_credits));
        }
        if let Some(canonical) = winners.get(&("title".into(), title_identity(candidate))) {
            candidate.title = canonical.value.clone();
        }
    }
    Ok(())
}

fn normalize_candidate_credits(candidate: &mut Candidate) {
    let credits = crate::domain::credits::normalize_structured(
        &candidate.artist,
        &candidate.title,
        std::mem::take(&mut candidate.artist_credits),
    );
    candidate.artist = credits.artist;
    candidate.title = credits.title;
    candidate.artist_credits = credits.artists;
    if let Some(album_artist) = candidate.album_artist.as_deref() {
        let credits = crate::domain::credits::normalize_structured(
            album_artist,
            "",
            std::mem::take(&mut candidate.album_artist_credits),
        );
        candidate.album_artist = Some(credits.artist);
        candidate.album_artist_credits = credits.artists;
    }
}

/// Enrich the candidate with the fuller source credit set so the stored,
/// displayed, and written ARTIST/ARTISTS match the source file's performers.
pub(crate) fn merge_source_credits(
    candidate: &mut Candidate,
    source_artist: Option<&str>,
    source_title: Option<&str>,
) {
    let merged = crate::domain::credits::finalize_credits(
        &candidate.artist,
        &candidate.title,
        std::mem::take(&mut candidate.artist_credits),
        source_artist,
        source_title,
    );
    candidate.artist = merged.artist;
    candidate.title = merged.title;
    candidate.artist_credits = merged.artists;
}

async fn collect_known_names(
    winners: &HashMap<(String, String), Canonical>,
    pool: &SqlitePool,
) -> Result<HashSet<String>> {
    let mut names = winners
        .keys()
        .filter(|(kind, _)| kind == "artist")
        .filter_map(|(_, key)| key.strip_prefix("name:"))
        .map(str::to_owned)
        .collect::<HashSet<_>>();
    let rows = sqlx::query_as::<_, (String,)>(
        "SELECT identity_key FROM canonical_names WHERE kind='artist' AND identity_key LIKE 'name:%'",
    )
    .fetch_all(pool)
    .await?;
    for (key,) in rows {
        if let Some(name) = key.strip_prefix("name:") {
            names.insert(name.to_owned());
        }
    }
    Ok(names)
}

/// Split a bare "X & Y" credit into separate artists only when the catalog
/// evidence identifies each side as a real artist and the whole name is not a
/// registered artist (an MBID-bearing group such as "Selena Gomez & the Scene").
/// Unverifiable pairs stay whole rather than blindly splitting group names.
fn resolve_ampersand_credits(credits: &mut Vec<ArtistCredit>, known_names: &HashSet<String>) {
    let mut out = Vec::with_capacity(credits.len());
    for credit in credits.drain(..) {
        if credit.musicbrainz_id.is_some() {
            out.push(credit);
            continue;
        }
        let Some(sides) = known_ampersand_split(&credit.name, known_names) else {
            out.push(credit);
            continue;
        };
        let total = sides.len();
        for (index, name) in sides.into_iter().enumerate() {
            let join = if index + 1 < total { " & " } else { "" };
            out.push(ArtistCredit::new(name, join));
        }
    }
    *credits = out;
}

fn known_ampersand_split(name: &str, known_names: &HashSet<String>) -> Option<Vec<String>> {
    if !name.contains(" & ") {
        return None;
    }
    let sides = name
        .split(" & ")
        .map(clean_text)
        .filter(|side| !side.is_empty())
        .collect::<Vec<_>>();
    if sides.len() < 2 {
        return None;
    }
    sides
        .iter()
        .all(|side| known_names.contains(&identity_key(side)))
        .then_some(sides)
}

fn collect_credits(
    winners: &mut HashMap<(String, String), Canonical>,
    kind: &str,
    credits: &[ArtistCredit],
    authority: i64,
) {
    for credit in credits {
        for key in artist_identities(credit) {
            winners
                .entry((kind.to_owned(), key))
                .and_modify(|current| {
                    if authority > current.authority {
                        current.value = credit.name.clone();
                        current.authority = authority;
                    }
                })
                .or_insert_with(|| Canonical {
                    value: credit.name.clone(),
                    authority,
                });
        }
    }
}

fn apply_credits(credits: &mut [ArtistCredit], winners: &HashMap<(String, String), Canonical>) {
    for credit in credits {
        if let Some(canonical) = artist_identities(credit)
            .into_iter()
            .filter_map(|key| winners.get(&("artist".into(), key)))
            .max_by_key(|canonical| canonical.authority)
        {
            credit.name = canonical.value.clone();
        }
    }
}

fn artist_identities(credit: &ArtistCredit) -> Vec<String> {
    let mut keys = vec![format!("name:{}", identity_key(&credit.name))];
    if let Some(id) = credit.musicbrainz_id.as_deref() {
        keys.insert(0, format!("mbid:{}", id.trim().to_ascii_lowercase()));
    }
    keys
}

fn title_identity(candidate: &Candidate) -> String {
    if let Some(id) = candidate.recording_id.as_deref() {
        return format!("mbid:{}", id.trim().to_ascii_lowercase());
    }
    if let Some(isrc) = candidate.isrc.as_deref() {
        return format!("isrc:{}", isrc.trim().to_ascii_uppercase());
    }
    format!(
        "text:{}|{}",
        identity_key(&candidate.artist),
        identity_key(&candidate.title)
    )
}

fn provider_authority(provider: &str) -> i64 {
    match provider {
        "musicbrainz" => 100,
        "spotify" => 95,
        "itunes" | "deezer" => 90,
        "shazam" | "audd" | "genius" => 80,
        "manual" => 70,
        _ => 60,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn trusted_spelling_wins_and_persists() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("canonical.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let mut candidates = vec![
            Candidate {
                provider: "genius".into(),
                title: "SONG".into(),
                artist: "ANDY".into(),
                artist_credits: vec![ArtistCredit::new("ANDY", "")],
                isrc: Some("IR-TEST".into()),
                ..Default::default()
            },
            Candidate {
                provider: "musicbrainz".into(),
                title: "Song".into(),
                artist: "Andy".into(),
                artist_credits: vec![ArtistCredit::new("Andy", "")],
                isrc: Some("IR-TEST".into()),
                ..Default::default()
            },
        ];
        canonicalize_candidates(&pool, &mut candidates)
            .await
            .unwrap();
        assert!(
            candidates
                .iter()
                .all(|candidate| candidate.artist == "Andy")
        );
        assert!(candidates.iter().all(|candidate| candidate.title == "Song"));
    }

    #[tokio::test]
    async fn one_band_identity_uses_the_same_catalog_spelling_everywhere() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("band-canonical.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let mut candidates = vec![Candidate {
            provider: "musicbrainz".into(),
            title: "Love You Like a Love Song".into(),
            artist: "Selena Gomez & The Scene".into(),
            artist_credits: vec![ArtistCredit::new("Selena Gomez & the Scene", "")],
            album_artist: Some("Selena Gomez & The Scene".into()),
            album_artist_credits: vec![ArtistCredit::new("Selena Gomez & The Scene", "")],
            ..Default::default()
        }];

        canonicalize_candidates(&pool, &mut candidates)
            .await
            .unwrap();

        let candidate = &candidates[0];
        assert_eq!(candidate.artist, "Selena Gomez & the Scene");
        assert_eq!(
            candidate.album_artist.as_deref(),
            Some("Selena Gomez & the Scene")
        );
        assert_eq!(candidate.artist_credits.len(), 1);
        assert_eq!(candidate.album_artist_credits.len(), 1);
    }

    #[tokio::test]
    async fn ampersand_pair_splits_when_both_sides_are_known_artists() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("pair.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        for (name, key) in [("Alice", "name:alice"), ("Bob", "name:bob")] {
            sqlx::query(
                "INSERT INTO canonical_names(kind,identity_key,canonical_value,authority,updated_at)
                 VALUES('artist',?,?,80,?)",
            )
            .bind(key)
            .bind(name)
            .bind(Utc::now().to_rfc3339())
            .execute(&pool)
            .await
            .unwrap();
        }
        let mut candidates = vec![Candidate {
            provider: "genius".into(),
            title: "Song".into(),
            artist: "Alice & Bob".into(),
            artist_credits: vec![ArtistCredit::new("Alice & Bob", "")],
            ..Default::default()
        }];
        canonicalize_candidates(&pool, &mut candidates)
            .await
            .unwrap();
        assert_eq!(candidates[0].artist, "Alice & Bob");
        assert_eq!(candidates[0].artist_credits.len(), 2);
        assert_eq!(candidates[0].artist_credits[0].name, "Alice");
        assert_eq!(candidates[0].artist_credits[1].name, "Bob");
    }

    #[tokio::test]
    async fn registered_group_with_ampersand_stays_whole() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("group.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let mut candidates = vec![Candidate {
            provider: "musicbrainz".into(),
            title: "Love You Like a Love Song".into(),
            artist: "Selena Gomez & The Scene".into(),
            artist_credits: vec![ArtistCredit {
                name: "Selena Gomez & the Scene".into(),
                join_phrase: String::new(),
                musicbrainz_id: Some("ae7572e2-2833-4864-b2e1-4c2c3e1e6b5f".into()),
            }],
            ..Default::default()
        }];
        canonicalize_candidates(&pool, &mut candidates)
            .await
            .unwrap();
        assert_eq!(candidates[0].artist, "Selena Gomez & the Scene");
        assert_eq!(candidates[0].artist_credits.len(), 1);
        assert_eq!(
            candidates[0].artist_credits[0].name,
            "Selena Gomez & the Scene"
        );
    }

    #[tokio::test]
    async fn unknown_ampersand_pair_stays_whole() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("unknown.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        let mut candidates = vec![Candidate {
            provider: "genius".into(),
            title: "The Sound of Silence".into(),
            artist: "Simon & Garfunkel".into(),
            artist_credits: vec![ArtistCredit::new("Simon & Garfunkel", "")],
            ..Default::default()
        }];
        canonicalize_candidates(&pool, &mut candidates)
            .await
            .unwrap();
        assert_eq!(candidates[0].artist, "Simon & Garfunkel");
        assert_eq!(candidates[0].artist_credits.len(), 1);
    }
}
