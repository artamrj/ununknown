use super::*;

pub async fn setup(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    let cfg = s.config.read().await;
    let reference_index = crate::application::reference_library::stats(&s.pool)
        .await
        .unwrap_or_default();
    let fpcalc = std::process::Command::new("fpcalc")
        .arg("-version")
        .output()
        .is_ok();
    let ffmpeg = crate::infrastructure::media::replaygain::available();
    let songrec = crate::infrastructure::providers::songrec::available();
    Json(serde_json::json!({
        "input_dir": cfg.input_dir,
        "output_dir": cfg.output_dir,
        "reference_dirs": cfg.reference_dirs,
        "reference_index": reference_index,
        "delete_source_after_write": cfg.delete_source_after_write,
        "automatic_scan_enabled": cfg.automatic_scan_enabled,
        "automatic_scan_interval_minutes": cfg.automatic_scan_interval_minutes,
        "sources": {
            "musicbrainz": true,
            "navahang": true,
            "audiomack": true,
            "itunes": true,
            "deezer": true,
            "radiojavan": true,
            "wikidata": true,
            "cover_art_archive": true,
            "fpcalc": fpcalc,
            "ffmpeg": ffmpeg,
            "songrec": songrec,
            "shazam": true,
            "integrity_check": crate::infrastructure::media::integrity::available(),
            "acoustid": !cfg.acoustid_key.is_empty(),
            "audd": !cfg.audd_token.is_empty(),
            "spotify": !cfg.spotify_client_id.is_empty() && !cfg.spotify_client_secret.is_empty(),
            "soundcloud": true,
            "soundcloud_search": !cfg.soundcloud_client_id.is_empty() && !cfg.soundcloud_client_secret.is_empty(),
            "youtube": !cfg.youtube_api_key.is_empty(),
            "discogs": !cfg.discogs_token.is_empty(),
            "lastfm": !cfg.lastfm_key.is_empty(),
            "genius": true,
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
    let mut reference_dirs = body
        .reference_dirs
        .unwrap_or_else(|| cfg.reference_dirs.clone());
    reference_dirs = reference_dirs
        .into_iter()
        .map(|path| path.trim().to_owned())
        .filter(|path| !path.is_empty())
        .collect();
    reference_dirs.sort();
    reference_dirs.dedup();
    let input_path = tokio::fs::canonicalize(input_dir).await?;
    let output_path = tokio::fs::canonicalize(output_dir).await?;
    for reference_dir in &reference_dirs {
        if !std::path::Path::new(reference_dir).is_dir() {
            return Err(ApiError::validation(format!(
                "Reference folder does not exist: {reference_dir}"
            )));
        }
        let reference_path = tokio::fs::canonicalize(reference_dir).await?;
        if paths_overlap(&reference_path, &input_path) {
            return Err(ApiError::validation(format!(
                "Reference folder must not overlap the input folder: {reference_dir}"
            )));
        }
        if paths_overlap(&reference_path, &output_path) {
            return Err(ApiError::validation(format!(
                "Reference folder must not overlap the output folder: {reference_dir}"
            )));
        }
    }
    let delete_source_after_write = body
        .delete_source_after_write
        .unwrap_or(cfg.delete_source_after_write);
    if delete_source_after_write {
        if input_path == output_path {
            return Err(ApiError::validation(
                "Input and output folders must be different when source removal is enabled",
            ));
        }
    }
    cfg.input_dir = input_dir.into();
    cfg.output_dir = output_dir.into();
    cfg.reference_dirs = reference_dirs;
    cfg.delete_source_after_write = delete_source_after_write;
    cfg.automatic_scan_enabled = body
        .automatic_scan_enabled
        .unwrap_or(cfg.automatic_scan_enabled);
    cfg.automatic_scan_interval_minutes = body
        .automatic_scan_interval_minutes
        .unwrap_or(cfg.automatic_scan_interval_minutes)
        .clamp(1, 24 * 60);
    if let Some(value) = body.acoustid_key.filter(|value| !value.trim().is_empty()) {
        cfg.acoustid_key = value.trim().into();
    }
    if let Some(value) = body.audd_token.filter(|value| !value.trim().is_empty()) {
        cfg.audd_token = value.trim().into();
    }
    if let Some(value) = body
        .spotify_client_id
        .filter(|value| !value.trim().is_empty())
    {
        cfg.spotify_client_id = value.trim().into();
    }
    if let Some(value) = body
        .spotify_client_secret
        .filter(|value| !value.trim().is_empty())
    {
        cfg.spotify_client_secret = value.trim().into();
    }
    if let Some(value) = body
        .soundcloud_client_id
        .filter(|value| !value.trim().is_empty())
    {
        cfg.soundcloud_client_id = value.trim().into();
    }
    if let Some(value) = body
        .soundcloud_client_secret
        .filter(|value| !value.trim().is_empty())
    {
        cfg.soundcloud_client_secret = value.trim().into();
    }
    if let Some(value) = body
        .youtube_api_key
        .filter(|value| !value.trim().is_empty())
    {
        cfg.youtube_api_key = value.trim().into();
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
    s.notify_automation_scheduler();
    Ok(Json(serde_json::json!({"saved": true})))
}

fn paths_overlap(left: &std::path::Path, right: &std::path::Path) -> bool {
    left.starts_with(right) || right.starts_with(left)
}

#[cfg(test)]
mod tests {
    use super::paths_overlap;

    #[test]
    fn nested_paths_overlap_in_either_direction() {
        let library = std::path::Path::new("/music/library");
        let album = std::path::Path::new("/music/library/album");
        assert!(paths_overlap(library, album));
        assert!(paths_overlap(album, library));
        assert!(!paths_overlap(
            library,
            std::path::Path::new("/music/inbox")
        ));
    }
}
