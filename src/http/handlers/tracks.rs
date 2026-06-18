use super::*;

pub async fn list_tracks(
    State(s): State<Arc<AppState>>,
    Query(q): Query<TrackQuery>,
) -> ApiResult<Json<TrackPage>> {
    let page = q.page.unwrap_or(1).max(1);
    let size = q.page_size.unwrap_or(100).clamp(20, 200);
    let status = q.status.unwrap_or_default();
    let search = format!("%{}%", q.search.unwrap_or_default());
    let total: i64 = sqlx::query_scalar("SELECT count(*) FROM tracks WHERE (?='' OR stage=?) AND (?='%%' OR filename LIKE ? OR current_title LIKE ? OR current_artist LIKE ?)")
        .bind(&status).bind(&status).bind(&search).bind(&search).bind(&search).bind(&search).fetch_one(&s.pool).await?;
    let tracks: Vec<Track> = sqlx::query_as("SELECT id,path,output_path,filename,format,duration,current_title,current_artist,current_album,current_album_artist,current_track_number,selected_candidate_id,status,error,is_missing,stage,stage_message,retry_count,next_retry_at FROM tracks WHERE (?='' OR stage=?) AND (?='%%' OR filename LIKE ? OR current_title LIKE ? OR current_artist LIKE ?) ORDER BY path LIMIT ? OFFSET ?")
        .bind(&status).bind(&status).bind(&search).bind(&search).bind(&search).bind(&search).bind(size).bind((page-1)*size).fetch_all(&s.pool).await?;
    let mut result = Vec::with_capacity(tracks.len());
    for track in tracks {
        let candidates = fetch_candidates(&s.pool, track.id).await?;
        result.push(WorkspaceTrack { track, candidates });
    }
    let rows: Vec<(String, i64)> =
        sqlx::query_as("SELECT stage,count(*) FROM tracks GROUP BY stage")
            .fetch_all(&s.pool)
            .await?;
    Ok(Json(TrackPage {
        items: result,
        total,
        counts: rows.into_iter().collect(),
    }))
}
pub async fn get_track(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
) -> ApiResult<Json<Track>> {
    Ok(Json(sqlx::query_as("SELECT id,path,output_path,filename,format,duration,current_title,current_artist,current_album,current_album_artist,current_track_number,selected_candidate_id,status,error,is_missing,stage,stage_message,retry_count,next_retry_at FROM tracks WHERE id=?").bind(id.0).fetch_one(&s.pool).await?))
}

pub async fn candidates(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
) -> ApiResult<Json<Vec<CandidateRow>>> {
    Ok(Json(fetch_candidates(&s.pool, id).await?))
}
pub async fn select_candidate(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
    Json(body): Json<SelectRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let result = sqlx::query("UPDATE tracks SET selected_candidate_id=?,status=CASE WHEN ? IS NULL THEN 'needs_review' ELSE 'selected' END WHERE id=?").bind(body.candidate_id.map(|v| v.0)).bind(body.candidate_id.map(|v| v.0)).bind(id.0).execute(&s.pool).await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("track not found"));
    }
    invalidate_previews(&s.pool).await?;
    Ok(Json(serde_json::json!({"selected":true})))
}
pub async fn edit_candidate(
    State(s): State<Arc<AppState>>,
    Path(id): Path<CandidateId>,
    Json(v): Json<CandidateEdit>,
) -> ApiResult<Json<serde_json::Value>> {
    let result = sqlx::query("UPDATE candidates SET title=?,artist=?,album=?,album_artist=?,track_number=?,track_total=?,disc_number=?,disc_total=?,year=?,genre=?,composer=?,label=?,isrc=?,provider='manual' WHERE id=?")
        .bind(v.title).bind(v.artist).bind(v.album).bind(v.album_artist).bind(v.track_number).bind(v.track_total).bind(v.disc_number).bind(v.disc_total).bind(v.year).bind(v.genre).bind(v.composer).bind(v.label).bind(v.isrc).bind(id.0).execute(&s.pool).await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("candidate not found"));
    }
    invalidate_previews(&s.pool).await?;
    Ok(Json(serde_json::json!({"saved":true})))
}
pub async fn retry_track(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
) -> ApiResult<Json<serde_json::Value>> {
    let result = sqlx::query(
        "UPDATE tracks SET file_mtime=-1,stage='discovered',status='new',error=NULL WHERE id=?",
    )
    .bind(id.0)
    .execute(&s.pool)
    .await?;
    if result.rows_affected() == 0 {
        return Err(ApiError::not_found("track not found"));
    }
    start_scan(State(s)).await
}
pub async fn retry_failed(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    sqlx::query("UPDATE tracks SET file_mtime=-1,stage='discovered',status='new',error=NULL WHERE stage='failed'").execute(&s.pool).await?;
    start_scan(State(s)).await
}
pub async fn skip_review(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    sqlx::query("UPDATE tracks SET selected_candidate_id=NULL,status='skipped',stage='skipped' WHERE stage='review'").execute(&s.pool).await?;
    Ok(Json(serde_json::json!({"skipped":true})))
}
