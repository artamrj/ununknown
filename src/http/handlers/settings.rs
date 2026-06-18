use super::*;

pub async fn settings(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::to_value(s.config.read().await.public()).unwrap())
}
pub async fn update_settings(
    State(s): State<Arc<AppState>>,
    Json(body): Json<SettingsRequest>,
) -> ApiResult<Json<serde_json::Value>> {
    let current = s.config.read().await.clone();
    let mut cfg = body.config;
    cfg.acoustid_api_key = current.acoustid_api_key;
    cfg.musicbrainz_user_agent = current.musicbrainz_user_agent;
    cfg.db_path = current.db_path;
    cfg.validate()?;
    crate::infrastructure::db::save_settings(&s.pool, &cfg).await?;
    s.refresh_limiters(&cfg).await;
    *s.config.write().await = cfg;
    invalidate_previews(&s.pool).await?;
    Ok(Json(serde_json::json!({"saved":true})))
}
pub async fn reset_settings(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    let current = s.config.read().await.clone();
    let mut cfg = Config {
        db_path: current.db_path,
        input_dir: current.input_dir,
        output_dir: current.output_dir,
        ..Default::default()
    };
    cfg.acoustid_api_key = current.acoustid_api_key;
    crate::infrastructure::db::save_settings(&s.pool, &cfg).await?;
    s.refresh_limiters(&cfg).await;
    *s.config.write().await = cfg;
    invalidate_previews(&s.pool).await?;
    Ok(Json(serde_json::json!({"reset":true})))
}

pub async fn reset_settings_section(
    State(s): State<Arc<AppState>>,
    Path(section): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let mut cfg = s.config.read().await.clone();
    let defaults = Config::default();
    match section.as_str() {
        "matching" => {
            cfg.automation_mode = defaults.automation_mode;
            cfg.confidence_threshold = defaults.confidence_threshold;
            cfg.track_attempts = defaults.track_attempts;
            cfg.scan_worker_concurrency = defaults.scan_worker_concurrency;
            cfg.metadata_read_concurrency = defaults.metadata_read_concurrency;
            cfg.fingerprint_concurrency = defaults.fingerprint_concurrency;
            cfg.acoustid_concurrency = defaults.acoustid_concurrency;
            cfg.artwork_download_concurrency = defaults.artwork_download_concurrency;
            cfg.tag_write_concurrency = defaults.tag_write_concurrency;
            cfg.db_write_batch_size = defaults.db_write_batch_size;
        }
        "metadata" => {
            cfg.metadata_fields = defaults.metadata_fields;
            cfg.overwrite_existing_tags = defaults.overwrite_existing_tags;
            cfg.cover_art_enabled = defaults.cover_art_enabled;
        }
        "files" => {
            cfg.path_templates = defaults.path_templates;
            cfg.in_place = defaults.in_place;
            cfg.output_mode = defaults.output_mode;
            cfg.expert_mode = false;
        }
        _ => return Err(anyhow!("unknown settings section").into()),
    }
    crate::infrastructure::db::save_settings(&s.pool, &cfg).await?;
    s.refresh_limiters(&cfg).await;
    *s.config.write().await = cfg;
    invalidate_previews(&s.pool).await?;
    Ok(Json(serde_json::json!({"reset":section})))
}
pub async fn test_acoustid(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    let cfg = s.config.read().await.clone();
    if cfg.acoustid_api_key.is_empty() {
        return Err(anyhow!("AcoustID is not configured").into());
    }
    Ok(Json(
        serde_json::json!({"ok":true,"message":"AcoustID key is configured. It will be validated during matching."}),
    ))
}
pub async fn test_musicbrainz(
    State(s): State<Arc<AppState>>,
) -> ApiResult<Json<serde_json::Value>> {
    let cfg = s.config.read().await.clone();
    crate::infrastructure::providers::musicbrainz::test_connection(
        &s.client,
        &cfg.musicbrainz_user_agent,
    )
    .await?;
    Ok(Json(
        serde_json::json!({"ok":true,"message":"MusicBrainz connection is working"}),
    ))
}
