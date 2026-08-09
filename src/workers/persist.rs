use crate::{
    core::{ActivityLogEntry, AppState},
    domain::audio,
    types::Candidate,
    workers::scan::{PersistJob, PreparedInput, file_snapshot, finish_scan_progress},
};
use anyhow::{Result, anyhow};
use chrono::Utc;
use std::{path::Path, sync::Arc};
use tokio::sync::mpsc;

pub(crate) async fn persist_duplicate_members(
    state: &Arc<AppState>,
    representative: &PreparedInput,
    duplicates: Vec<PreparedInput>,
    total: usize,
) {
    for duplicate in duplicates {
        if state.workflow_cancelled().await {
            return;
        }
        let filename = duplicate
            .path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("audio")
            .to_owned();
        let result = persist_input_duplicate(&state.pool, &duplicate, &representative.path).await;
        match result {
            Ok(()) => {
                state
                    .log_entry(
                        ActivityLogEntry::new(
                            "ok",
                            "deduplicate",
                            "Skipped duplicate input before online lookup",
                        )
                        .file(filename)
                        .detail(format!(
                            "Best input kept: {}",
                            representative.path.display()
                        )),
                    )
                    .await;
            }
            Err(error) => {
                state.increment_failed().await;
                state
                    .log_entry(
                        ActivityLogEntry::new(
                            "error",
                            "deduplicate",
                            "Could not store duplicate input result",
                        )
                        .file(filename)
                        .error(error.as_ref()),
                    )
                    .await;
            }
        }
        finish_scan_progress(state, total).await;
    }
}

pub(crate) async fn db_writer(
    state: Arc<AppState>,
    mut rx: mpsc::Receiver<PersistJob>,
    batch_size: usize,
) -> Result<()> {
    let batch_size = batch_size.max(1);
    while let Some(first) = rx.recv().await {
        let mut batch = vec![first];
        while batch.len() < batch_size {
            match rx.try_recv() {
                Ok(job) => batch.push(job),
                Err(_) => break,
            }
        }
        let batch_result: anyhow::Result<()> = async {
            let mut tx = state.pool.begin().await?;
            for job in &batch {
                persist_match(&mut tx, &job.path, &job.info, &job.candidate, &job.message).await?;
            }
            tx.commit().await?;
            Ok(())
        }
        .await;
        let error_msg = batch_result.as_ref().err().map(|e| format!("{e:#}"));
        if let Some(msg) = &error_msg {
            state
                .log_entry(
                    ActivityLogEntry::new("error", "db", "Failed to persist batch")
                        .error_text(msg.clone()),
                )
                .await;
        }
        for job in batch {
            let result = match &error_msg {
                None => Ok(()),
                Some(msg) => Err(anyhow::anyhow!("{msg}")),
            };
            let _ = job.result.send(result);
        }
    }
    Ok(())
}

pub(crate) async fn persist_match(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    path: &Path,
    info: &audio::AudioInfo,
    c: &Candidate,
    message: &str,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let text = path.to_string_lossy();
    let filename = path
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or_else(|| anyhow!("invalid filename"))?;
    let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM tracks WHERE path=?")
        .bind(text.as_ref())
        .fetch_optional(&mut **tx)
        .await?;
    let id = if let Some(id) = existing {
        sqlx::query("DELETE FROM candidates WHERE track_id=?")
            .bind(id)
            .execute(&mut **tx)
            .await?;
        sqlx::query("UPDATE tracks SET filename=?,format=?,duration=?,current_title=?,current_artist=?,current_album=?,current_album_artist=?,current_track_number=?,status='selected',error=NULL,is_missing=0,last_seen_at=?,last_scanned_at=?,stage='ready',stage_message=?,updated_at=?,selected_candidate_id=NULL WHERE id=?")
            .bind(filename).bind(&info.format).bind(info.duration).bind(&info.title).bind(&info.artist).bind(&info.album).bind(&info.album_artist).bind(info.track_number.map(i64::from)).bind(&now).bind(&now).bind(message).bind(&now).bind(id).execute(&mut **tx).await?;
        id
    } else {
        sqlx::query("INSERT INTO tracks(path,filename,format,duration,current_title,current_artist,current_album,current_album_artist,current_track_number,status,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage,stage_message,updated_at) VALUES(?,?,?,?,?,?,?,?,?,'selected',0,?,?,?,'ready',?,?)")
            .bind(text.as_ref()).bind(filename).bind(&info.format).bind(info.duration).bind(&info.title).bind(&info.artist).bind(&info.album).bind(&info.album_artist).bind(info.track_number.map(i64::from)).bind(&now).bind(&now).bind(&now).bind(message).bind(&now).execute(&mut **tx).await?.last_insert_rowid()
    };
    let cid = insert_candidate(tx, id, c).await?;
    sqlx::query("UPDATE tracks SET selected_candidate_id=? WHERE id=?")
        .bind(cid)
        .bind(id)
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(crate) async fn insert_candidate(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    track_id: i64,
    c: &Candidate,
) -> Result<i64> {
    let candidate_id = sqlx::query("INSERT INTO candidates(track_id,provider,title,artist,album,album_artist,track_number,track_total,disc_number,disc_total,year,genre,composer,label,isrc,cover_url,musicbrainz_recording_id,musicbrainz_release_id,release_country,release_date,release_type,release_secondary_types,is_compilation,duration_delta,score_breakdown,musicbrainz_artist_id,musicbrainz_album_artist_id,score,raw_json) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(track_id)
        .bind(if c.provider.is_empty() {
            "musicbrainz"
        } else {
            c.provider.as_str()
        })
        .bind(&c.title)
        .bind(&c.artist)
        .bind(&c.album)
        .bind(&c.album_artist)
        .bind(c.track_number)
        .bind(c.track_total)
        .bind(c.disc_number)
        .bind(c.disc_total)
        .bind(&c.year)
        .bind(&c.genre)
        .bind(&c.composer)
        .bind(&c.label)
        .bind(&c.isrc)
        .bind(&c.cover_url)
        .bind(&c.recording_id)
        .bind(&c.release_id)
        .bind(&c.release_country)
        .bind(&c.release_date)
        .bind(&c.release_type)
        .bind(&c.release_secondary_types)
        .bind(c.is_compilation)
        .bind(c.duration_delta)
        .bind(&c.score_breakdown)
        .bind(&c.artist_id)
        .bind(&c.album_artist_id)
        .bind(c.score)
        .bind(&c.raw_json)
        .execute(&mut **tx)
        .await?
        .last_insert_rowid();
    sqlx::query(
        "UPDATE candidates SET artist_credits_json=?,album_artist_credits_json=?,
         artwork_candidates_json=?,artwork_status=?,artwork_message=? WHERE id=?",
    )
    .bind(serde_json::to_string(&c.artist_credits)?)
    .bind(serde_json::to_string(&c.album_artist_credits)?)
    .bind(serde_json::to_string(&c.artwork_candidates)?)
    .bind(
        serde_json::to_value(&c.artwork_status)?
            .as_str()
            .unwrap_or("searching"),
    )
    .bind(&c.artwork_message)
    .bind(candidate_id)
    .execute(&mut **tx)
    .await?;
    insert_candidate_source(
        tx,
        candidate_id,
        provider_display_name(if c.provider.is_empty() {
            "musicbrainz"
        } else {
            c.provider.as_str()
        }),
        Some(c.score),
        serde_json::json!({
            "reason": provider_reason(if c.provider.is_empty() { "musicbrainz" } else { c.provider.as_str() }),
            "recording_id": c.recording_id,
            "release_id": c.release_id
        }),
        Some(&c.raw_json),
    )
    .await?;
    if let Some(score_breakdown) = &c.score_breakdown
        && score_breakdown.contains("\"acoustid\"")
    {
        insert_candidate_source(
            tx,
            candidate_id,
            "AcoustID",
            Some(c.score),
            serde_json::json!({
                "reason": "Fingerprint evidence contributed to score",
                "breakdown": score_breakdown
            }),
            None,
        )
        .await?;
    }
    Ok(candidate_id)
}

pub(crate) fn provider_display_name(provider: &str) -> &str {
    match provider {
        "acoustid" => "AcoustID",
        "discogs" => "Discogs",
        "deezer" => "Deezer",
        "audd" => "AudD",
        "audiomack" => "Audiomack",
        "navahang" => "Navahang",
        "shazam" => "Shazam",
        "itunes" => "Apple Music",
        "lastfm" => "Last.fm",
        "genius" => "Genius",
        "radiojavan" => "Radio Javan",
        "songrec" => "SongRec / Shazam",
        "soundcloud" => "SoundCloud",
        "spotify" => "Spotify",
        "theaudiodb" => "TheAudioDB",
        "wikidata" => "Wikidata",
        "youtube" => "YouTube",
        _ => "MusicBrainz",
    }
}

pub(crate) fn provider_reason(provider: &str) -> &str {
    match provider {
        "discogs" => "Release, label, catalog, and physical media metadata",
        "deezer" => "International track, album, ISRC, duration, and cover metadata",
        "audd" => "Audio recognition with ISRC and linked catalog metadata",
        "audiomack" => {
            "Song identity, original artwork, duration, album, genre, release date, credits, label, and ISRC when supplied"
        }
        "navahang" => {
            "Persian song identity, duration, release date, credits, label, and original artwork"
        }
        "shazam" => {
            "Verified song identity, ISRC, album, release, credits, duration, and high-resolution artwork"
        }
        "lastfm" => "Track popularity, tags, and MusicBrainz ID evidence",
        "genius" => "Song identity, credited artist, album, release date, and song artwork",
        "radiojavan" => "Persian track title, artist, duration, release date, and original artwork",
        "songrec" => {
            "Audio fingerprint recognition through Shazam, with catalog enrichment for metadata and ISRC"
        }
        "soundcloud" => "Creator-uploaded title, artist, genre, date, duration, and artwork",
        "spotify" => "ISRC, release, track position, duration, and cover metadata",
        "theaudiodb" => "Track, album, genre, and image enrichment",
        "wikidata" => "Structured identifier and external-link evidence",
        "youtube" => "Exact source-video title, channel, date, and duration evidence",
        _ => "Canonical recording and release metadata",
    }
}

pub(crate) async fn insert_candidate_source(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    candidate_id: i64,
    provider: &str,
    confidence: Option<f64>,
    reason: serde_json::Value,
    raw_json: Option<&str>,
) -> Result<()> {
    sqlx::query("INSERT INTO candidate_sources(candidate_id,provider,confidence,reason_json,raw_json,created_at) VALUES(?,?,?,?,?,?)")
        .bind(candidate_id)
        .bind(provider)
        .bind(confidence)
        .bind(reason.to_string())
        .bind(raw_json)
        .bind(Utc::now().to_rfc3339())
        .execute(&mut **tx)
        .await?;
    Ok(())
}

pub(crate) async fn persist_review(
    pool: &sqlx::SqlitePool,
    path: &Path,
    info: &audio::AudioInfo,
    candidates: &[Candidate],
    message: &str,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let id = upsert_track_outcome(
        &mut tx,
        path,
        Some(info),
        "needs_review",
        "review",
        Some(message),
        None,
    )
    .await?;
    sqlx::query("DELETE FROM candidates WHERE track_id=?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    for candidate in candidates.iter().take(5) {
        insert_candidate(&mut tx, id, candidate).await?;
    }
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn persist_unmatched(
    pool: &sqlx::SqlitePool,
    path: &Path,
    info: &audio::AudioInfo,
    message: &str,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let id = upsert_track_outcome(
        &mut tx,
        path,
        Some(info),
        "needs_review",
        "review",
        Some(message),
        None,
    )
    .await?;
    sqlx::query("DELETE FROM candidates WHERE track_id=?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn persist_failed(
    pool: &sqlx::SqlitePool,
    path: &Path,
    error: &str,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let id = upsert_track_outcome(
        &mut tx,
        path,
        None,
        "provider_error",
        "failed",
        Some("Track failed after retries"),
        Some(error),
    )
    .await?;
    sqlx::query("DELETE FROM candidates WHERE track_id=?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn persist_corrupt(
    pool: &sqlx::SqlitePool,
    path: &Path,
    info: &audio::AudioInfo,
    diagnostic: &str,
) -> Result<()> {
    let mut tx = pool.begin().await?;
    let id = upsert_track_outcome(
        &mut tx,
        path,
        Some(info),
        "corrupt",
        "failed",
        Some("Audio file is damaged and cannot be written safely"),
        Some(diagnostic),
    )
    .await?;
    sqlx::query("DELETE FROM candidates WHERE track_id=?")
        .bind(id)
        .execute(&mut *tx)
        .await?;
    tx.commit().await?;
    Ok(())
}

pub(crate) async fn persist_analysis_identity(
    pool: &sqlx::SqlitePool,
    input: &PreparedInput,
) -> Result<()> {
    let snapshot = file_snapshot(&input.path).await;
    sqlx::query(
        "UPDATE tracks SET bitrate=?,file_size=?,file_mtime=?,content_fingerprint=?,updated_at=?
         WHERE path=?",
    )
    .bind(
        input
            .info
            .as_ref()
            .and_then(|info| info.bitrate)
            .map(i64::from),
    )
    .bind(snapshot.map(|value| value.0))
    .bind(snapshot.map(|value| value.1))
    .bind(&input.content_key)
    .bind(Utc::now().to_rfc3339())
    .bind(input.path.to_string_lossy().as_ref())
    .execute(pool)
    .await?;
    Ok(())
}

pub(crate) async fn persist_input_duplicate(
    pool: &sqlx::SqlitePool,
    input: &PreparedInput,
    representative: &Path,
) -> Result<()> {
    let message = format!("Duplicate of input file: {}", representative.display());
    let mut transaction = pool.begin().await?;
    let id = upsert_track_outcome(
        &mut transaction,
        &input.path,
        input.info.as_ref(),
        "duplicate",
        "skipped",
        Some(&message),
        None,
    )
    .await?;
    let snapshot = file_snapshot(&input.path).await;
    sqlx::query(
        "UPDATE tracks SET output_path=?,bitrate=?,file_size=?,file_mtime=?,content_fingerprint=?
         WHERE id=?",
    )
    .bind(representative.to_string_lossy().as_ref())
    .bind(
        input
            .info
            .as_ref()
            .and_then(|info| info.bitrate)
            .map(i64::from),
    )
    .bind(snapshot.map(|value| value.0))
    .bind(snapshot.map(|value| value.1))
    .bind(&input.content_key)
    .bind(id)
    .execute(&mut *transaction)
    .await?;
    sqlx::query("DELETE FROM candidates WHERE track_id=?")
        .bind(id)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(())
}

pub(crate) async fn upsert_track_outcome(
    tx: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    path: &Path,
    info: Option<&audio::AudioInfo>,
    status: &str,
    stage: &str,
    stage_message: Option<&str>,
    error: Option<&str>,
) -> Result<i64> {
    let now = Utc::now().to_rfc3339();
    let text = path.to_string_lossy();
    let filename = path
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or_else(|| anyhow!("invalid filename"))?;
    let format = info
        .map(|info| info.format.clone())
        .or_else(|| {
            path.extension()
                .and_then(|extension| extension.to_str())
                .map(str::to_ascii_lowercase)
        })
        .unwrap_or_default();
    let existing: Option<i64> = sqlx::query_scalar("SELECT id FROM tracks WHERE path=?")
        .bind(text.as_ref())
        .fetch_optional(&mut **tx)
        .await?;
    if let Some(id) = existing {
        sqlx::query("UPDATE tracks SET filename=?,format=?,duration=?,current_title=?,current_artist=?,current_album=?,current_album_artist=?,current_track_number=?,selected_candidate_id=NULL,status=?,error=?,is_missing=0,last_seen_at=?,last_scanned_at=?,stage=?,stage_message=?,updated_at=? WHERE id=?")
            .bind(filename)
            .bind(&format)
            .bind(info.map(|info| info.duration))
            .bind(info.and_then(|info| info.title.as_ref()))
            .bind(info.and_then(|info| info.artist.as_ref()))
            .bind(info.and_then(|info| info.album.as_ref()))
            .bind(info.and_then(|info| info.album_artist.as_ref()))
            .bind(info.and_then(|info| info.track_number.map(i64::from)))
            .bind(status)
            .bind(error)
            .bind(&now)
            .bind(&now)
            .bind(stage)
            .bind(stage_message)
            .bind(&now)
            .bind(id)
            .execute(&mut **tx)
            .await?;
        Ok(id)
    } else {
        let id = sqlx::query("INSERT INTO tracks(path,filename,format,duration,current_title,current_artist,current_album,current_album_artist,current_track_number,selected_candidate_id,status,error,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage,stage_message,updated_at) VALUES(?,?,?,?,?,?,?,?,?,NULL,?,?,0,?,?,?,?,?,?)")
            .bind(text.as_ref())
            .bind(filename)
            .bind(&format)
            .bind(info.map(|info| info.duration))
            .bind(info.and_then(|info| info.title.as_ref()))
            .bind(info.and_then(|info| info.artist.as_ref()))
            .bind(info.and_then(|info| info.album.as_ref()))
            .bind(info.and_then(|info| info.album_artist.as_ref()))
            .bind(info.and_then(|info| info.track_number.map(i64::from)))
            .bind(status)
            .bind(error)
            .bind(&now)
            .bind(&now)
            .bind(&now)
            .bind(stage)
            .bind(stage_message)
            .bind(&now)
            .execute(&mut **tx)
            .await?
            .last_insert_rowid();
        Ok(id)
    }
}
