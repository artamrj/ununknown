use super::*;

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

pub async fn select_candidate(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
    Json(body): Json<SelectRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let candidate_id = body
        .candidate_id
        .ok_or_else(|| ApiError::validation("candidate is required"))?;
    let belongs: bool =
        sqlx::query_scalar("SELECT EXISTS(SELECT 1 FROM candidates WHERE id=? AND track_id=?)")
            .bind(candidate_id.0)
            .bind(id.0)
            .fetch_one(&s.pool)
            .await?;
    if !belongs {
        return Err(ApiError::not_found("candidate not found for this track"));
    }
    sqlx::query("UPDATE tracks SET selected_candidate_id=?,status='selected',stage='ready',stage_message=NULL WHERE id=?")
        .bind(candidate_id.0)
        .bind(id.0)
        .execute(&s.pool)
        .await?;
    Ok(Json(serde_json::json!({"selected": true})))
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
        ApiError::validation("Enter a valid HTTPS image, Spotify, or SoundCloud track URL")
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
    let parsed = reqwest::Url::parse(url)
        .map_err(|_| ApiError::validation("Enter a valid Spotify, SoundCloud, or YouTube URL"))?;
    let candidate = if parsed.host_str() == Some("open.spotify.com") {
        crate::infrastructure::providers::spotify::lookup_url(&s.client, url).await
    } else if is_soundcloud_host(parsed.host_str()) {
        crate::infrastructure::providers::soundcloud::lookup_url(&s.client, url).await
    } else {
        crate::infrastructure::providers::youtube::lookup_url(&s.client, url).await
    }
    .map_err(|error| ApiError::validation(format!("Source lookup failed: {error:#}")))?;
    Ok(Json(candidate))
}

fn is_soundcloud_host(host: Option<&str>) -> bool {
    matches!(host, Some("soundcloud.com" | "www.soundcloud.com"))
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
}
