use super::*;

pub async fn current_artwork(
    State(s): State<Arc<AppState>>,
    Path(id): Path<TrackId>,
) -> ApiResult<Response> {
    let path: String = sqlx::query_scalar("SELECT path FROM tracks WHERE id=?")
        .bind(id.0)
        .fetch_one(&s.pool)
        .await?;
    let artwork =
        tokio::task::spawn_blocking(move || crate::domain::audio::artwork(&PathBuf::from(path)))
            .await
            .map_err(|error| anyhow!("could not read artwork: {error}"))?
            .map_err(|_| ApiError::not_found("current artwork not available"))?;
    let Some(artwork) = artwork else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    Ok(image_response(artwork.mime, artwork.data))
}
pub async fn proposed_artwork(
    State(s): State<Arc<AppState>>,
    Path(id): Path<CandidateId>,
) -> ApiResult<Response> {
    let row: Option<(Option<String>, Option<String>)> =
        sqlx::query_as("SELECT cover_url,musicbrainz_release_id FROM candidates WHERE id=?")
            .bind(id.0)
            .fetch_optional(&s.pool)
            .await?;
    let (cover_url, release_id) = row.unwrap_or_default();
    let Some(url) = cover_url else {
        return Ok(StatusCode::NOT_FOUND.into_response());
    };
    let cache_dir = PathBuf::from(&s.config.read().await.db_path)
        .parent()
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("/cache"))
        .join("artwork");
    tokio::fs::create_dir_all(&cache_dir).await?;
    let cache_path = cache_dir.join(format!("candidate-{id}.img"));
    let mime_path = cache_dir.join(format!("candidate-{id}.mime"));
    if let Ok(data) = tokio::fs::read(&cache_path).await {
        let mime = tokio::fs::read_to_string(&mime_path)
            .await
            .unwrap_or_else(|_| "image/jpeg".into());
        return Ok(image_response(mime, data));
    }
    let limiter = s.artwork_downloads.read().await.clone();
    let _permit = limiter.acquire_owned().await?;
    let downloaded = if let Some(release_id) = release_id.as_deref() {
        (
            "image/jpeg".to_owned(),
            crate::infrastructure::providers::cover_art_archive::fetch_cached(
                &s.pool, &s.client, release_id, &url,
            )
            .await
            .map_err(|_| ApiError::not_found("proposed artwork not available"))?,
        )
    } else {
        let response = s
            .client
            .get(url)
            .send()
            .await
            .map_err(|_| ApiError::not_found("proposed artwork not available"))?
            .error_for_status()
            .map_err(|_| ApiError::not_found("proposed artwork not available"))?;
        let mime = response
            .headers()
            .get(header::CONTENT_TYPE)
            .and_then(|value| value.to_str().ok())
            .unwrap_or("image/jpeg")
            .to_owned();
        let data = response
            .bytes()
            .await
            .map_err(|_| ApiError::not_found("proposed artwork not available"))?
            .to_vec();
        (mime, data)
    };
    let (mime, data) = downloaded;
    tokio::fs::write(&cache_path, &data).await?;
    tokio::fs::write(&mime_path, &mime).await?;
    Ok(image_response(mime, data))
}
pub(super) fn image_response(mime: impl AsRef<str>, data: Vec<u8>) -> Response {
    Response::builder()
        .header(header::CONTENT_TYPE, mime.as_ref())
        .header(header::CACHE_CONTROL, "public, max-age=86400")
        .body(Body::from(data))
        .unwrap_or_else(|_| StatusCode::INTERNAL_SERVER_ERROR.into_response())
}
