use super::*;

pub async fn setup(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let cfg = s.config.read().await;
    let fpcalc = std::process::Command::new("fpcalc")
        .arg("-version")
        .output()
        .is_ok();
    Json(serde_json::json!({
        "input_dir": cfg.input_dir,
        "output_dir": cfg.output_dir,
        "sources": {
            "musicbrainz": true,
            "itunes": true,
            "wikidata": true,
            "cover_art_archive": true,
            "fpcalc": fpcalc,
            "acoustid": !cfg.acoustid_key.is_empty(),
            "discogs": !cfg.discogs_token.is_empty(),
            "lastfm": !cfg.lastfm_key.is_empty(),
            "theaudiodb": !cfg.theaudiodb_key.is_empty()
        }
    }))
}

pub async fn update_setup(
    State(s): State<Arc<AppState>>,
    Json(body): Json<SetupRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let input_dir = body.input_dir.trim();
    let output_dir = body.output_dir.trim();
    if input_dir.is_empty() || output_dir.is_empty() {
        return Err(ApiError::validation(
            "Input and output folders are required",
        ));
    }
    if !std::path::Path::new(input_dir).is_dir() {
        return Err(ApiError::validation("Input folder does not exist"));
    }
    tokio::fs::create_dir_all(output_dir).await?;

    let mut cfg = s.config.read().await.clone();
    cfg.input_dir = input_dir.into();
    cfg.output_dir = output_dir.into();
    if let Some(value) = body.acoustid_key.filter(|value| !value.trim().is_empty()) {
        cfg.acoustid_key = value.trim().into();
    }
    if let Some(value) = body.discogs_token.filter(|value| !value.trim().is_empty()) {
        cfg.discogs_token = value.trim().into();
    }
    if let Some(value) = body.lastfm_key.filter(|value| !value.trim().is_empty()) {
        cfg.lastfm_key = value.trim().into();
    }
    if let Some(value) = body.theaudiodb_key.filter(|value| !value.trim().is_empty()) {
        cfg.theaudiodb_key = value.trim().into();
    }
    crate::infrastructure::db::save_settings(&s.pool, &cfg).await?;
    *s.config.write().await = cfg;
    Ok(Json(serde_json::json!({"saved": true})))
}
