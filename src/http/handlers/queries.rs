use super::*;

pub(super) const TRACK_FIELDS: &str = "id,path,output_path,filename,format,bitrate,duration,content_fingerprint,current_title,current_artist,current_album,current_album_artist,current_track_number,selected_candidate_id,status,error,is_missing,stage,stage_message,retry_count,next_retry_at";

pub(super) async fn track(pool: &sqlx::SqlitePool, id: TrackId) -> ApiResult<Track> {
    Ok(sqlx::query_as(sqlx::AssertSqlSafe(format!(
        "SELECT {TRACK_FIELDS} FROM tracks WHERE id=?"
    )))
    .bind(id.0)
    .fetch_one(pool)
    .await?)
}

pub(super) async fn selected(
    pool: &sqlx::SqlitePool,
    id: TrackId,
) -> ApiResult<(Track, Candidate)> {
    let track = track(pool, id).await?;
    let cid = track
        .selected_candidate_id
        .ok_or_else(|| ApiError::not_found("track has no selected candidate"))?;
    let row: CandidateRow = sqlx::query_as("SELECT * FROM candidates WHERE id=?")
        .bind(cid.0)
        .fetch_one(pool)
        .await?;
    Ok((track, row.value()))
}

pub(super) async fn selected_for_tracks(
    pool: &sqlx::SqlitePool,
    tracks: Vec<Track>,
) -> ApiResult<Vec<(Track, Candidate)>> {
    let rows: Vec<CandidateRow> = sqlx::query_as(
        "SELECT candidates.* FROM candidates
         JOIN tracks ON tracks.selected_candidate_id=candidates.id
         WHERE tracks.selected_candidate_id IS NOT NULL",
    )
    .fetch_all(pool)
    .await?;
    let mut candidates = rows
        .into_iter()
        .map(|row| (row.id.0, row.value()))
        .collect::<std::collections::HashMap<_, _>>();
    let mut out = Vec::with_capacity(tracks.len());
    for track in tracks {
        let candidate_id = track
            .selected_candidate_id
            .ok_or_else(|| ApiError::not_found("track has no selected candidate"))?;
        let candidate = candidates
            .remove(&candidate_id.0)
            .ok_or_else(|| ApiError::not_found("selected candidate no longer exists"))?;
        out.push((track, candidate));
    }
    Ok(out)
}

pub(super) async fn candidates(pool: &sqlx::SqlitePool, id: TrackId) -> Result<Vec<CandidateRow>> {
    Ok(
        sqlx::query_as("SELECT * FROM candidates WHERE track_id=? ORDER BY score DESC")
            .bind(id.0)
            .fetch_all(pool)
            .await?,
    )
}
