use super::*;
use axum::http::{HeaderMap, StatusCode, header};
use tokio::io::{AsyncReadExt, AsyncSeekExt};

pub async fn list_tracks(State(s): State<Arc<AppState>>) -> ApiResult<Json<TrackPage>> {
    let tracks: Vec<Track> = sqlx::query_as(&format!(
        "SELECT {} FROM tracks ORDER BY path LIMIT 10000",
        queries::TRACK_FIELDS
    ))
    .fetch_all(&s.pool)
    .await?;
    let total = tracks.len() as i64;
    let mut items = Vec::with_capacity(tracks.len());
    for track in tracks {
        let mut candidates = queries::candidates(&s.pool, track.id).await?;
        for candidate in &mut candidates {
            candidate.normalize_credits();
        }
        items.push(WorkspaceTrack { track, candidates });
    }
    Ok(Json(TrackPage { items, total }))
}

pub async fn auto_approve_review(
    State(s): State<Arc<AppState>>,
) -> ApiResult<Json<AutoApproveResult>> {
    if s.workflow_running().await {
        return Err(ApiError::conflict(
            "Wait for the current scan or write operation to finish",
        ));
    }

    let tracks: Vec<Track> = sqlx::query_as(&format!(
        "SELECT {} FROM tracks WHERE stage='review' AND selected_candidate_id IS NULL ORDER BY id",
        queries::TRACK_FIELDS
    ))
    .fetch_all(&s.pool)
    .await?;
    let candidate_rows: Vec<CandidateRow> = sqlx::query_as(
        "SELECT candidates.* FROM candidates
         JOIN tracks ON tracks.id=candidates.track_id
         WHERE tracks.stage='review' AND tracks.selected_candidate_id IS NULL
         ORDER BY candidates.track_id, candidates.id",
    )
    .fetch_all(&s.pool)
    .await?;
    let mut candidates_by_track = std::collections::HashMap::new();
    for row in candidate_rows {
        candidates_by_track
            .entry(row.track_id.0)
            .or_insert_with(Vec::new)
            .push(row.value());
    }

    let mut decisions = Vec::new();
    let mut low_confidence = 0_u64;
    let mut unavailable = 0_u64;
    for track in tracks {
        if track.status == "corrupt" || track.is_missing {
            unavailable += 1;
            continue;
        }
        let candidates = candidates_by_track.remove(&track.id.0).unwrap_or_default();
        if candidates.is_empty() {
            unavailable += 1;
            continue;
        }
        let evidence = crate::application::smart_approval::TrackEvidence {
            filename: &track.filename,
            title: track.current_title.as_deref(),
            artist: track.current_artist.as_deref(),
            album: track.current_album.as_deref(),
        };
        if let Some(decision) = crate::application::smart_approval::select(evidence, &candidates) {
            let Some(mut selected) = candidates
                .iter()
                .find(|candidate| candidate.id == Some(decision.candidate_id))
                .cloned()
            else {
                unavailable += 1;
                continue;
            };
            let path = std::path::PathBuf::from(&track.path);
            let embedded_cover = tokio::task::spawn_blocking(move || {
                crate::infrastructure::media::tag_writer::read_artwork(&path)
                    .ok()
                    .flatten()
                    .is_some()
            })
            .await
            .unwrap_or(false);
            crate::application::metadata_completion::complete(
                &mut selected,
                &candidates,
                track.current_album.as_deref(),
                embedded_cover,
            );
            let limiter = s.artwork_downloads.read().await.clone();
            let _permit = limiter.acquire_owned().await.map_err(anyhow::Error::from)?;
            crate::application::metadata_completion::ensure_usable_cover(
                &s.pool,
                &s.client,
                &mut selected,
                embedded_cover,
            )
            .await;
            let completion =
                crate::application::metadata_completion::reassess(&mut selected, embedded_cover);
            if completion.core_complete {
                decisions.push((track.id, decision, selected, completion));
            } else {
                low_confidence += 1;
            }
        } else {
            low_confidence += 1;
        }
    }

    let mut transaction = s.pool.begin().await?;
    let updated_at = chrono::Utc::now().to_rfc3339();
    let mut approved = 0_u64;
    for (track_id, decision, candidate, completion) in decisions {
        let message = format!(
            "Smart auto-approved ({:.0}%): {}; {}",
            decision.confidence,
            decision.explanation,
            completion.summary()
        );
        persist_completed_candidate(&mut transaction, &candidate).await?;
        approved += sqlx::query(
            "UPDATE tracks
             SET selected_candidate_id=?,status='selected',stage='ready',stage_message=?,updated_at=?
             WHERE id=? AND stage='review' AND selected_candidate_id IS NULL",
        )
        .bind(decision.candidate_id)
        .bind(message)
        .bind(&updated_at)
        .bind(track_id.0)
        .execute(&mut *transaction)
        .await?
        .rows_affected();
    }
    let remaining: i64 = sqlx::query_scalar(
        "SELECT count(*) FROM tracks WHERE stage='review' AND selected_candidate_id IS NULL",
    )
    .fetch_one(&mut *transaction)
    .await?;
    transaction.commit().await?;

    Ok(Json(AutoApproveResult {
        approved,
        remaining,
        low_confidence,
        unavailable,
    }))
}

pub async fn track_audio(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
    headers: HeaderMap,
) -> ApiResult<axum::response::Response> {
    let track = queries::track(&s.pool, id).await?;
    let path = std::path::Path::new(&track.path);
    let metadata = tokio::fs::metadata(path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            ApiError::not_found("audio source file no longer exists")
        } else {
            ApiError::from(error)
        }
    })?;
    let total = metadata.len();
    if total == 0 {
        return Err(ApiError::validation("audio source file is empty"));
    }
    let requested = match parse_byte_range(
        headers
            .get(header::RANGE)
            .and_then(|value| value.to_str().ok()),
        total,
    ) {
        Ok(range) => range,
        Err(()) => {
            return Ok(axum::response::Response::builder()
                .status(StatusCode::RANGE_NOT_SATISFIABLE)
                .header(header::CONTENT_RANGE, format!("bytes */{total}"))
                .body(axum::body::Body::empty())
                .map_err(anyhow::Error::from)?);
        }
    };
    let (start, end, status) = requested.map_or((0, total - 1, StatusCode::OK), |(start, end)| {
        (start, end, StatusCode::PARTIAL_CONTENT)
    });
    let length = usize::try_from(end - start + 1)
        .map_err(|_| ApiError::validation("requested audio range is too large"))?;
    let mut file = tokio::fs::File::open(path).await?;
    file.seek(std::io::SeekFrom::Start(start)).await?;
    let mut bytes = vec![0_u8; length];
    file.read_exact(&mut bytes).await?;

    let mut response = axum::response::Response::builder()
        .status(status)
        .header(header::CONTENT_TYPE, audio_mime(&track))
        .header(header::ACCEPT_RANGES, "bytes")
        .header(header::CONTENT_LENGTH, length.to_string())
        .header(header::CACHE_CONTROL, "private, no-store");
    if status == StatusCode::PARTIAL_CONTENT {
        response = response.header(
            header::CONTENT_RANGE,
            format!("bytes {start}-{end}/{total}"),
        );
    }
    Ok(response
        .body(axum::body::Body::from(bytes))
        .map_err(anyhow::Error::from)?)
}

fn parse_byte_range(
    value: Option<&str>,
    total: u64,
) -> std::result::Result<Option<(u64, u64)>, ()> {
    let Some(value) = value else {
        return Ok(None);
    };
    let value = value.strip_prefix("bytes=").ok_or(())?;
    if value.contains(',') {
        return Err(());
    }
    let (start, end) = value.split_once('-').ok_or(())?;
    if start.is_empty() {
        let suffix = end.parse::<u64>().map_err(|_| ())?;
        if suffix == 0 {
            return Err(());
        }
        return Ok(Some((total.saturating_sub(suffix), total - 1)));
    }
    let start = start.parse::<u64>().map_err(|_| ())?;
    if start >= total {
        return Err(());
    }
    let end = if end.is_empty() {
        total - 1
    } else {
        end.parse::<u64>().map_err(|_| ())?.min(total - 1)
    };
    if end < start {
        return Err(());
    }
    Ok(Some((start, end)))
}

fn audio_mime(track: &Track) -> &'static str {
    match track.format.as_deref().unwrap_or_default() {
        "mp3" => "audio/mpeg",
        "m4a" | "mp4" | "aac" => "audio/mp4",
        "flac" => "audio/flac",
        "ogg" => "audio/ogg",
        "opus" => "audio/ogg; codecs=opus",
        "wav" => "audio/wav",
        "aiff" | "aif" => "audio/aiff",
        _ => "application/octet-stream",
    }
}

pub async fn select_candidate(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
    Json(body): Json<SelectRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let candidate_id = body
        .candidate_id
        .ok_or_else(|| ApiError::validation("candidate is required"))?;
    let track = queries::track(&s.pool, id).await?;
    let rows = queries::candidates(&s.pool, id).await?;
    let candidates = rows.iter().map(CandidateRow::value).collect::<Vec<_>>();
    let Some(mut selected) = candidates
        .iter()
        .find(|candidate| candidate.id == Some(candidate_id.0))
        .cloned()
    else {
        return Err(ApiError::not_found("candidate not found for this track"));
    };
    let source_path = std::path::PathBuf::from(&track.path);
    let embedded_cover = tokio::task::spawn_blocking(move || {
        crate::infrastructure::media::tag_writer::read_artwork(&source_path)
            .ok()
            .flatten()
            .is_some()
    })
    .await
    .unwrap_or(false);
    crate::application::metadata_completion::complete(
        &mut selected,
        &candidates,
        track.current_album.as_deref(),
        embedded_cover,
    );
    let limiter = s.artwork_downloads.read().await.clone();
    let _permit = limiter.acquire_owned().await.map_err(anyhow::Error::from)?;
    crate::application::metadata_completion::ensure_usable_cover(
        &s.pool,
        &s.client,
        &mut selected,
        embedded_cover,
    )
    .await;
    let completion =
        crate::application::metadata_completion::reassess(&mut selected, embedded_cover);
    let mut transaction = s.pool.begin().await?;
    persist_completed_candidate(&mut transaction, &selected).await?;
    sqlx::query("UPDATE tracks SET selected_candidate_id=?,status='selected',stage='ready',stage_message=?,updated_at=? WHERE id=?")
        .bind(candidate_id.0)
        .bind(format!("Selected by you; {}", completion.summary()))
        .bind(chrono::Utc::now().to_rfc3339())
        .bind(id.0)
        .execute(&mut *transaction)
        .await?;
    transaction.commit().await?;
    Ok(Json(serde_json::json!({"selected": true})))
}

async fn persist_completed_candidate(
    transaction: &mut sqlx::Transaction<'_, sqlx::Sqlite>,
    candidate: &Candidate,
) -> Result<()> {
    let id = candidate
        .id
        .ok_or_else(|| anyhow::anyhow!("completed candidate has no database ID"))?;
    sqlx::query(
        "UPDATE candidates SET title=?,artist=?,album=?,album_artist=?,track_number=?,track_total=?,disc_number=?,disc_total=?,year=?,genre=?,composer=?,label=?,isrc=?,cover_url=?,release_country=?,release_date=?,release_type=?,release_secondary_types=?,is_compilation=?,score_breakdown=? WHERE id=?",
    )
    .bind(&candidate.title)
    .bind(&candidate.artist)
    .bind(&candidate.album)
    .bind(&candidate.album_artist)
    .bind(candidate.track_number)
    .bind(candidate.track_total)
    .bind(candidate.disc_number)
    .bind(candidate.disc_total)
    .bind(&candidate.year)
    .bind(&candidate.genre)
    .bind(&candidate.composer)
    .bind(&candidate.label)
    .bind(&candidate.isrc)
    .bind(&candidate.cover_url)
    .bind(&candidate.release_country)
    .bind(&candidate.release_date)
    .bind(&candidate.release_type)
    .bind(&candidate.release_secondary_types)
    .bind(candidate.is_compilation)
    .bind(&candidate.score_breakdown)
    .bind(id)
    .execute(&mut **transaction)
    .await?;
    Ok(())
}

pub async fn return_to_review(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
) -> ApiResult<Json<serde_json::Value>> {
    if s.workflow_running().await {
        return Err(ApiError::conflict(
            "Wait for the current scan or write operation to finish",
        ));
    }
    let track = queries::track(&s.pool, id).await?;
    if track.selected_candidate_id.is_none() {
        return Err(ApiError::validation(
            "This track does not have an identification to undo",
        ));
    }
    let message = if track.output_path.is_some() {
        "Returned to review; the existing corrected output was not removed"
    } else {
        "Returned to review"
    };
    let changed = sqlx::query(
        "UPDATE tracks
         SET selected_candidate_id=NULL,status='needs_review',stage='review',stage_message=?,updated_at=?
         WHERE id=? AND selected_candidate_id IS NOT NULL",
    )
    .bind(message)
    .bind(chrono::Utc::now().to_rfc3339())
    .bind(id.0)
    .execute(&s.pool)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(ApiError::conflict(
            "The track identification was already changed",
        ));
    }
    Ok(Json(serde_json::json!({"returned_to_review": true})))
}

pub async fn manual_candidate(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
    Json(mut value): Json<CandidateEdit>,
) -> ApiResult<Json<serde_json::Value>> {
    if value.title.trim().is_empty() || value.artist.trim().is_empty() {
        return Err(ApiError::validation("Title and artist are required"));
    }
    value.artist = crate::domain::credits::prefer_latin_alias(&value.artist);
    value.album_artist = value
        .album_artist
        .as_deref()
        .map(crate::domain::credits::prefer_latin_alias);
    let credits = crate::domain::credits::normalize_featured(&value.artist, &value.title);
    value.artist = credits.artist;
    value.title = credits.title;
    let track: Option<(String, String)> =
        sqlx::query_as("SELECT status,path FROM tracks WHERE id=?")
            .bind(id.0)
            .fetch_optional(&s.pool)
            .await?;
    let Some((status, track_path)) = track else {
        return Err(ApiError::not_found("track not found"));
    };
    if status == "corrupt" {
        return Err(ApiError::validation(
            "Damaged audio cannot be marked ready; repair or replace the source file first",
        ));
    }
    let cover_url = value
        .cover_url
        .as_deref()
        .map(str::trim)
        .filter(|url| !url.is_empty())
        .map(str::to_owned);
    let result = sqlx::query("INSERT INTO candidates(track_id,provider,title,artist,album,album_artist,track_number,track_total,disc_number,disc_total,year,genre,composer,label,isrc,cover_url,score,raw_json) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(id.0).bind("manual").bind(value.title.trim()).bind(value.artist.trim())
        .bind(value.album).bind(value.album_artist).bind(value.track_number).bind(value.track_total)
        .bind(value.disc_number).bind(value.disc_total).bind(value.year).bind(value.genre)
        .bind(value.composer).bind(value.label).bind(value.isrc)
        .bind(&cover_url)
        .bind(100.0).bind("{}")
        .execute(&s.pool).await?;
    if let Some(cover_url) = cover_url {
        persist_artwork_override(
            &s.pool,
            &track_path,
            value.title.trim(),
            value.artist.trim(),
            &cover_url,
        )
        .await?;
    }
    let candidate_id = result.last_insert_rowid();
    sqlx::query("UPDATE tracks SET selected_candidate_id=?,status='selected',stage='ready',stage_message='Entered manually' WHERE id=?")
        .bind(candidate_id).bind(id.0).execute(&s.pool).await?;
    Ok(Json(
        serde_json::json!({"selected": true, "candidate_id": candidate_id}),
    ))
}

pub async fn update_artwork(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
    Json(value): Json<ArtworkEdit>,
) -> ApiResult<Json<serde_json::Value>> {
    let (track, selected) = queries::selected(&s.pool, id).await?;
    let supplied = value.cover_url.trim();
    let parsed = reqwest::Url::parse(supplied).map_err(|_| {
        ApiError::validation(
            "Enter a valid HTTPS image, Spotify, SoundCloud, Radio Javan, or Genius song URL",
        )
    })?;
    if parsed.scheme() != "https" {
        return Err(ApiError::validation("Cover URLs must use HTTPS"));
    }
    let cover_url = if parsed.host_str() == Some("open.spotify.com") {
        crate::infrastructure::providers::spotify::lookup_url(&s.client, supplied)
            .await
            .map_err(|error| ApiError::validation(format!("Spotify link failed: {error:#}")))?
            .cover_url
            .ok_or_else(|| ApiError::validation("Spotify did not return cover artwork"))?
    } else if is_soundcloud_host(parsed.host_str()) {
        crate::infrastructure::providers::soundcloud::lookup_url(&s.client, supplied)
            .await
            .map_err(|error| ApiError::validation(format!("SoundCloud link failed: {error:#}")))?
            .cover_url
            .ok_or_else(|| ApiError::validation("SoundCloud did not return cover artwork"))?
    } else if is_radiojavan_host(parsed.host_str()) {
        crate::infrastructure::providers::radiojavan::lookup_url(&s.pool, &s.client, supplied)
            .await
            .map_err(|error| ApiError::validation(format!("Radio Javan link failed: {error:#}")))?
            .cover_url
            .ok_or_else(|| ApiError::validation("Radio Javan did not return cover artwork"))?
    } else if is_genius_host(parsed.host_str()) {
        let access_token = s.config.read().await.genius_access_token.clone();
        crate::infrastructure::providers::genius::lookup_url(
            &s.pool,
            &s.client,
            &access_token,
            supplied,
        )
        .await
        .map_err(|error| ApiError::validation(format!("Genius link failed: {error:#}")))?
        .cover_url
        .ok_or_else(|| ApiError::validation("Genius did not return cover artwork"))?
    } else {
        supplied.to_owned()
    };
    let bytes = crate::infrastructure::providers::cover_art_archive::fetch(&s.client, &cover_url)
        .await
        .map_err(|error| ApiError::validation(format!("Cover download failed: {error:#}")))?;
    tag_writer::validate_artwork(&bytes).map_err(|error| {
        ApiError::validation(format!("The URL is not a valid image: {error:#}"))
    })?;
    let changed = sqlx::query(
        "UPDATE candidates SET cover_url=? WHERE id=(SELECT selected_candidate_id FROM tracks WHERE id=?)",
    )
    .bind(&cover_url)
    .bind(id.0)
    .execute(&s.pool)
    .await?
    .rows_affected();
    if changed == 0 {
        return Err(ApiError::not_found(
            "Select metadata for this track before setting its cover",
        ));
    }
    persist_artwork_override(
        &s.pool,
        &track.path,
        &selected.title,
        &selected.artist,
        &cover_url,
    )
    .await?;
    Ok(Json(serde_json::json!({"cover_url": cover_url})))
}

async fn persist_artwork_override(
    pool: &sqlx::SqlitePool,
    path: &str,
    title: &str,
    artist: &str,
    cover_url: &str,
) -> Result<()> {
    sqlx::query("INSERT INTO artwork_overrides(path,title,artist,cover_url,updated_at) VALUES(?,?,?,?,?) ON CONFLICT(path) DO UPDATE SET title=excluded.title,artist=excluded.artist,cover_url=excluded.cover_url,updated_at=excluded.updated_at")
        .bind(path)
        .bind(title)
        .bind(artist)
        .bind(cover_url)
        .bind(chrono::Utc::now().to_rfc3339())
        .execute(pool)
        .await?;
    Ok(())
}

pub async fn artwork_preview(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
) -> ApiResult<axum::response::Response> {
    let (track, candidate) = queries::selected(&s.pool, id).await?;
    let artwork = match super::apply::resolve_artwork(&s, &track.filename, &candidate).await? {
        Some(bytes) => Some(bytes),
        None => tokio::task::spawn_blocking(move || {
            tag_writer::read_artwork(std::path::Path::new(&track.path))
        })
        .await
        .map_err(anyhow::Error::from)??,
    }
    .ok_or_else(|| ApiError::not_found("No valid artwork is available for this track"))?;
    artwork_response(artwork)
}

pub async fn candidate_artwork_preview(
    State(s): State<Arc<AppState>>,
    Path(id): Path<CandidateId>,
) -> ApiResult<axum::response::Response> {
    let row: CandidateRow = sqlx::query_as("SELECT * FROM candidates WHERE id=?")
        .bind(id.0)
        .fetch_optional(&s.pool)
        .await?
        .ok_or_else(|| ApiError::not_found("candidate not found"))?;
    let candidate = row.value();
    let mut urls = candidate.cover_url.into_iter().collect::<Vec<_>>();
    if let Some(value) = candidate
        .score_breakdown
        .as_deref()
        .and_then(|raw| serde_json::from_str::<serde_json::Value>(raw).ok())
    {
        for item in value["artwork_candidates"].as_array().into_iter().flatten() {
            if let Some(url) = item["url"].as_str()
                && !urls.iter().any(|existing| existing == url)
            {
                urls.push(url.to_owned());
            }
        }
    }
    for url in urls {
        if let Ok(bytes) = crate::infrastructure::providers::cover_art_archive::fetch_url_cached(
            &s.pool, &s.client, &url,
        )
        .await
            && tag_writer::validate_artwork(&bytes).is_ok()
        {
            return artwork_response(bytes);
        }
    }
    Err(ApiError::not_found(
        "No valid catalog artwork is available for this candidate",
    ))
}

fn artwork_response(artwork: Vec<u8>) -> ApiResult<axum::response::Response> {
    let mime = artwork_mime(&artwork);
    Ok(axum::response::Response::builder()
        .header(axum::http::header::CONTENT_TYPE, mime)
        .header(axum::http::header::CACHE_CONTROL, "private, max-age=300")
        .body(axum::body::Body::from(artwork))
        .map_err(anyhow::Error::from)?)
}

fn artwork_mime(data: &[u8]) -> &'static str {
    if data.starts_with(b"\x89PNG\r\n\x1a\n") {
        "image/png"
    } else if data.starts_with(b"GIF8") {
        "image/gif"
    } else if data.starts_with(b"RIFF") && data.get(8..12) == Some(b"WEBP") {
        "image/webp"
    } else {
        "image/jpeg"
    }
}

pub async fn resolve_source(
    State(s): State<Arc<AppState>>,
    Json(value): Json<SourceLookupRequest>,
) -> ApiResult<Json<Candidate>> {
    let url = value.url.trim();
    let parsed = reqwest::Url::parse(url).map_err(|_| {
        ApiError::validation(
            "Enter a valid Spotify, SoundCloud, Radio Javan, Genius, or YouTube URL",
        )
    })?;
    let candidate = if parsed.host_str() == Some("open.spotify.com") {
        crate::infrastructure::providers::spotify::lookup_url(&s.client, url).await
    } else if is_soundcloud_host(parsed.host_str()) {
        crate::infrastructure::providers::soundcloud::lookup_url(&s.client, url).await
    } else if is_radiojavan_host(parsed.host_str()) {
        crate::infrastructure::providers::radiojavan::lookup_url(&s.pool, &s.client, url).await
    } else if is_genius_host(parsed.host_str()) {
        let access_token = s.config.read().await.genius_access_token.clone();
        crate::infrastructure::providers::genius::lookup_url(&s.pool, &s.client, &access_token, url)
            .await
    } else {
        crate::infrastructure::providers::youtube::lookup_url(&s.client, url).await
    }
    .map_err(|error| ApiError::validation(format!("Source lookup failed: {error:#}")))?;
    Ok(Json(candidate))
}

fn is_soundcloud_host(host: Option<&str>) -> bool {
    matches!(host, Some("soundcloud.com" | "www.soundcloud.com"))
}

fn is_radiojavan_host(host: Option<&str>) -> bool {
    matches!(
        host,
        Some("play.radiojavan.com" | "www.play.radiojavan.com")
    )
}

fn is_genius_host(host: Option<&str>) -> bool {
    matches!(host, Some("genius.com" | "www.genius.com"))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::{config::Config, infrastructure::db};

    #[tokio::test]
    async fn manual_metadata_resolves_a_completely_unmatched_track() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("manual.sqlite");
        let pool = db::connect(database.to_str().unwrap()).await.unwrap();
        let state = Arc::new(AppState::new(Config::default(), pool.clone()));
        let track_id = sqlx::query("INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage) VALUES('/music/unknown.mp3','unknown.mp3','needs_review',0,'now','now','now','review')")
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();

        let _ = manual_candidate(
            State(state),
            Path(TrackId(track_id)),
            Json(CandidateEdit {
                title: "Correct title".into(),
                artist: "Correct artist".into(),
                album: Some("Correct album".into()),
                album_artist: None,
                track_number: Some(1),
                track_total: None,
                disc_number: None,
                disc_total: None,
                year: Some("2026".into()),
                genre: None,
                composer: None,
                label: None,
                isrc: None,
                cover_url: Some("https://example.test/cover.jpg".into()),
            }),
        )
        .await
        .unwrap();

        let row: (String, String, String) = sqlx::query_as("SELECT tracks.stage,candidates.title,candidates.artist FROM tracks JOIN candidates ON candidates.id=tracks.selected_candidate_id WHERE tracks.id=?")
            .bind(track_id)
            .fetch_one(&pool)
            .await
            .unwrap();
        assert_eq!(
            row,
            (
                "ready".into(),
                "Correct title".into(),
                "Correct artist".into()
            )
        );
    }

    #[tokio::test]
    async fn auto_approve_uses_smart_selection_and_leaves_unsafe_tracks() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("auto-approve.sqlite");
        let pool = db::connect(database.to_str().unwrap()).await.unwrap();
        let state = Arc::new(AppState::new(Config::default(), pool.clone()));
        crate::infrastructure::provider_cache::ProviderCache::put(
            &pool,
            "artwork-url",
            &crate::infrastructure::provider_cache::search_key(
                "https://example.test/cover.jpg",
            ),
            &serde_json::json!({
                "data_base64": "iVBORw0KGgoAAAANSUhEUgAAAAEAAAABCAQAAAC1HAwCAAAAC0lEQVR42mNk+A8AAQUBAScY42YAAAAASUVORK5CYII="
            }),
            chrono::Utc::now() + chrono::Duration::days(1),
        )
        .await
        .unwrap();

        let review_id = insert_test_track(&pool, "/music/review.mp3", "needs_review", 0).await;
        let smart_id = insert_test_candidate(&pool, review_id, "Song", 90.0).await;
        let highest_id = insert_test_candidate(&pool, review_id, "Song (Live)", 99.0).await;
        let corrupt_id = insert_test_track(&pool, "/music/corrupt.mp3", "corrupt", 0).await;
        insert_test_candidate(&pool, corrupt_id, "Must stay", 99.0).await;
        insert_test_track(&pool, "/music/unmatched.mp3", "needs_review", 0).await;
        let missing_id = insert_test_track(&pool, "/music/missing.mp3", "needs_review", 1).await;
        insert_test_candidate(&pool, missing_id, "Missing source", 99.0).await;

        let result = auto_approve_review(State(state)).await.unwrap().0;

        assert_eq!(result.approved, 1);
        assert_eq!(result.remaining, 3);
        assert_eq!(result.low_confidence, 0);
        assert_eq!(result.unavailable, 3);
        let selected: (Option<i64>, String, String, Option<String>) = sqlx::query_as(
            "SELECT selected_candidate_id,status,stage,stage_message FROM tracks WHERE id=?",
        )
        .bind(review_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(selected.0, Some(smart_id));
        assert_ne!(selected.0, Some(highest_id));
        assert_eq!(selected.1, "selected");
        assert_eq!(selected.2, "ready");
        assert!(selected.3.unwrap().starts_with("Smart auto-approved"));
        let untouched: i64 = sqlx::query_scalar(
            "SELECT count(*) FROM tracks WHERE id IN (?,?) AND stage='review' AND selected_candidate_id IS NULL",
        )
        .bind(corrupt_id)
        .bind(missing_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(untouched, 2);
    }

    #[tokio::test]
    async fn return_to_review_clears_selection_but_keeps_candidates() {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("return-to-review.sqlite");
        let pool = db::connect(database.to_str().unwrap()).await.unwrap();
        let state = Arc::new(AppState::new(Config::default(), pool.clone()));
        let track_id = sqlx::query("INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage) VALUES('/music/ready.mp3','ready.mp3','selected',0,'now','now','now','ready')")
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
        let candidate_id = sqlx::query("INSERT INTO candidates(track_id,provider,title,artist,score) VALUES(?,'deezer','Song','Artist',95)")
            .bind(track_id)
            .execute(&pool)
            .await
            .unwrap()
            .last_insert_rowid();
        sqlx::query("UPDATE tracks SET selected_candidate_id=? WHERE id=?")
            .bind(candidate_id)
            .bind(track_id)
            .execute(&pool)
            .await
            .unwrap();

        let _ = return_to_review(State(state), Path(TrackId(track_id)))
            .await
            .unwrap();

        let track: (Option<i64>, String, String, Option<String>) = sqlx::query_as(
            "SELECT selected_candidate_id,status,stage,stage_message FROM tracks WHERE id=?",
        )
        .bind(track_id)
        .fetch_one(&pool)
        .await
        .unwrap();
        assert_eq!(track.0, None);
        assert_eq!(track.1, "needs_review");
        assert_eq!(track.2, "review");
        assert_eq!(track.3.as_deref(), Some("Returned to review"));
        let candidates: i64 =
            sqlx::query_scalar("SELECT count(*) FROM candidates WHERE track_id=?")
                .bind(track_id)
                .fetch_one(&pool)
                .await
                .unwrap();
        assert_eq!(candidates, 1);
    }

    async fn insert_test_track(
        pool: &sqlx::SqlitePool,
        path: &str,
        status: &str,
        is_missing: i64,
    ) -> i64 {
        sqlx::query("INSERT INTO tracks(path,filename,current_title,current_artist,status,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage) VALUES(?,?,'Song','Artist',?,?,'now','now','now','review')")
            .bind(path)
            .bind(path.rsplit('/').next().unwrap())
            .bind(status)
            .bind(is_missing)
            .execute(pool)
            .await
            .unwrap()
            .last_insert_rowid()
    }

    async fn insert_test_candidate(
        pool: &sqlx::SqlitePool,
        track_id: i64,
        title: &str,
        score: f64,
    ) -> i64 {
        sqlx::query("INSERT INTO candidates(track_id,provider,title,artist,album,cover_url,duration_delta,score,score_breakdown) VALUES(?,'deezer',?,'Artist','Album','https://example.test/cover.jpg',1.0,?,'{\"sources\":[\"Deezer\"]}')")
            .bind(track_id)
            .bind(title)
            .bind(score)
            .execute(pool)
            .await
            .unwrap()
            .last_insert_rowid()
    }

    #[test]
    fn parses_browser_audio_ranges() {
        assert_eq!(parse_byte_range(None, 100), Ok(None));
        assert_eq!(parse_byte_range(Some("bytes=0-9"), 100), Ok(Some((0, 9))));
        assert_eq!(parse_byte_range(Some("bytes=90-"), 100), Ok(Some((90, 99))));
        assert_eq!(parse_byte_range(Some("bytes=-10"), 100), Ok(Some((90, 99))));
        assert!(parse_byte_range(Some("bytes=100-"), 100).is_err());
        assert!(parse_byte_range(Some("items=0-9"), 100).is_err());
    }
}
