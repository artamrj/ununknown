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
        let candidates = queries::candidates(&s.pool, track.id).await?;
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
    Json(value): Json<CandidateEdit>,
) -> ApiResult<Json<serde_json::Value>> {
    if value.title.trim().is_empty() || value.artist.trim().is_empty() {
        return Err(ApiError::validation("Title and artist are required"));
    }
    let status: Option<String> = sqlx::query_scalar("SELECT status FROM tracks WHERE id=?")
        .bind(id.0)
        .fetch_optional(&s.pool)
        .await?;
    let Some(status) = status else {
        return Err(ApiError::not_found("track not found"));
    };
    if status == "corrupt" {
        return Err(ApiError::validation(
            "Damaged audio cannot be marked ready; repair or replace the source file first",
        ));
    }
    let result = sqlx::query("INSERT INTO candidates(track_id,provider,title,artist,album,album_artist,track_number,track_total,disc_number,disc_total,year,genre,composer,label,isrc,cover_url,score,raw_json) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(id.0).bind("manual").bind(value.title.trim()).bind(value.artist.trim())
        .bind(value.album).bind(value.album_artist).bind(value.track_number).bind(value.track_total)
        .bind(value.disc_number).bind(value.disc_total).bind(value.year).bind(value.genre)
        .bind(value.composer).bind(value.label).bind(value.isrc)
        .bind(value.cover_url.filter(|url| !url.trim().is_empty()))
        .bind(100.0).bind("{}")
        .execute(&s.pool).await?;
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
    let supplied = value.cover_url.trim();
    let parsed = reqwest::Url::parse(supplied)
        .map_err(|_| ApiError::validation("Enter a valid HTTPS image or Spotify track URL"))?;
    if parsed.scheme() != "https" {
        return Err(ApiError::validation("Cover URLs must use HTTPS"));
    }
    let cover_url = if parsed.host_str() == Some("open.spotify.com") {
        let value = s
            .client
            .get("https://open.spotify.com/oembed")
            .query(&[("url", supplied)])
            .send()
            .await
            .and_then(reqwest::Response::error_for_status)
            .map_err(|error| ApiError::validation(format!("Spotify link failed: {error}")))?
            .json::<serde_json::Value>()
            .await
            .map_err(|error| ApiError::validation(format!("Spotify response failed: {error}")))?;
        value["thumbnail_url"]
            .as_str()
            .map(upgrade_spotify_image)
            .ok_or_else(|| ApiError::validation("Spotify did not return cover artwork"))?
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
    Ok(Json(serde_json::json!({"cover_url": cover_url})))
}

pub async fn resolve_source(
    State(s): State<Arc<AppState>>,
    Json(value): Json<SourceLookupRequest>,
) -> ApiResult<Json<Candidate>> {
    let candidate =
        crate::infrastructure::providers::youtube::lookup_url(&s.client, value.url.trim())
            .await
            .map_err(|error| ApiError::validation(format!("Source lookup failed: {error:#}")))?;
    Ok(Json(candidate))
}

fn upgrade_spotify_image(url: &str) -> String {
    url.replace("ab67616d00001e02", "ab67616d0000b273")
        .replace("ab67616d00004851", "ab67616d0000b273")
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

    #[test]
    fn spotify_thumbnail_is_upgraded_to_large_cover() {
        assert_eq!(
            upgrade_spotify_image("https://i.scdn.co/image/ab67616d00001e02abc"),
            "https://i.scdn.co/image/ab67616d0000b273abc"
        );
    }
}
