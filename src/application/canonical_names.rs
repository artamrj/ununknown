use crate::{
    domain::credits::{display_artist, identity_key},
    infrastructure::providers::{ArtistCredit, Candidate},
};
use anyhow::Result;
use chrono::Utc;
use sqlx::SqlitePool;
use std::collections::HashMap;

#[derive(Clone)]
struct Canonical {
    value: String,
    authority: i64,
}

pub async fn canonicalize_candidates(
    pool: &SqlitePool,
    candidates: &mut [Candidate],
) -> Result<()> {
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
}
