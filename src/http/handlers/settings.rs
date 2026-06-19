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
    cfg.db_path = current.db_path.clone();
    preserve_provider_secrets(&mut cfg, &current);
    sync_provider_aliases(&mut cfg);
    cfg.validate()
        .map_err(|error| ApiError::validation(error.to_string()))?;
    crate::infrastructure::db::save_settings(&s.pool, &cfg).await?;
    s.refresh_limiters(&cfg).await;
    *s.config.write().await = cfg;
    previews::invalidate(&s.pool).await?;
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
    cfg.metadata_sources.acoustid.api_key = current.metadata_sources.acoustid.api_key;
    cfg.metadata_sources.musicbrainz.user_agent = current.metadata_sources.musicbrainz.user_agent;
    sync_provider_aliases(&mut cfg);
    crate::infrastructure::db::save_settings(&s.pool, &cfg).await?;
    s.refresh_limiters(&cfg).await;
    *s.config.write().await = cfg;
    previews::invalidate(&s.pool).await?;
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
            cfg.matching_strategy = defaults.matching_strategy;
            cfg.compilation_preference = defaults.compilation_preference;
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
        "sources" => {
            let current_sources = cfg.metadata_sources.clone();
            cfg.metadata_sources = defaults.metadata_sources;
            cfg.metadata_sources.acoustid.api_key = current_sources.acoustid.api_key;
            cfg.metadata_sources.musicbrainz.user_agent = current_sources.musicbrainz.user_agent;
            cfg.metadata_sources.discogs.api_key = current_sources.discogs.api_key;
            cfg.metadata_sources.discogs.token = current_sources.discogs.token;
            cfg.metadata_sources.theaudiodb.api_key = current_sources.theaudiodb.api_key;
            cfg.metadata_sources.lastfm.api_key = current_sources.lastfm.api_key;
        }
        "files" => {
            cfg.path_templates = defaults.path_templates;
            cfg.in_place = defaults.in_place;
            cfg.output_mode = defaults.output_mode;
            cfg.expert_mode = false;
        }
        _ => return Err(ApiError::not_found("unknown settings section")),
    }
    sync_provider_aliases(&mut cfg);
    crate::infrastructure::db::save_settings(&s.pool, &cfg).await?;
    s.refresh_limiters(&cfg).await;
    *s.config.write().await = cfg;
    previews::invalidate(&s.pool).await?;
    Ok(Json(serde_json::json!({"reset":section})))
}

fn preserve_provider_secrets(cfg: &mut Config, current: &Config) {
    if cfg.metadata_sources.acoustid.api_key == "configured" {
        cfg.metadata_sources.acoustid.api_key = current.metadata_sources.acoustid.api_key.clone();
    }
    if cfg.metadata_sources.discogs.api_key == "configured" {
        cfg.metadata_sources.discogs.api_key = current.metadata_sources.discogs.api_key.clone();
    }
    if cfg.metadata_sources.discogs.token == "configured" {
        cfg.metadata_sources.discogs.token = current.metadata_sources.discogs.token.clone();
    }
    if cfg.metadata_sources.theaudiodb.api_key == "configured" {
        cfg.metadata_sources.theaudiodb.api_key =
            current.metadata_sources.theaudiodb.api_key.clone();
    }
    if cfg.metadata_sources.lastfm.api_key == "configured" {
        cfg.metadata_sources.lastfm.api_key = current.metadata_sources.lastfm.api_key.clone();
    }
    if cfg.metadata_sources.musicbrainz.user_agent == "configured" {
        cfg.metadata_sources.musicbrainz.user_agent =
            current.metadata_sources.musicbrainz.user_agent.clone();
    }
}

fn sync_provider_aliases(cfg: &mut Config) {
    cfg.acoustid_api_key = cfg.metadata_sources.acoustid.api_key.clone();
    cfg.musicbrainz_user_agent = cfg.metadata_sources.musicbrainz.user_agent.clone();
}
pub async fn test_acoustid(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    let cfg = s.config.read().await.clone();
    if cfg.acoustid_key().is_empty() {
        return Err(ApiError::forbidden("AcoustID is not configured"));
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
        cfg.musicbrainz_user_agent(),
    )
    .await
    .map_err(|error| {
        if !Config::valid_musicbrainz_user_agent(cfg.musicbrainz_user_agent()) {
            ApiError::forbidden(error.to_string())
        } else if let Some(reqwest_error) = error.downcast_ref::<reqwest::Error>() {
            if reqwest_error.is_timeout() {
                ApiError::timeout(reqwest_error.to_string())
            } else {
                ApiError::provider(reqwest_error.to_string())
            }
        } else {
            ApiError::provider(error.to_string())
        }
    })?;
    Ok(Json(
        serde_json::json!({"ok":true,"message":"MusicBrainz connection is working"}),
    ))
}

pub async fn provider_status(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    Json(serde_json::to_value(s.config.read().await.provider_statuses()).unwrap())
}

pub async fn test_provider(
    State(s): State<Arc<AppState>>,
    Path(provider): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let cfg = s.config.read().await.clone();
    let statuses = cfg.provider_statuses();
    let status = match provider.as_str() {
        "musicbrainz" => statuses.musicbrainz.status,
        "acoustid" => statuses.acoustid.status,
        "discogs" => statuses.discogs.status,
        "cover_art_archive" => statuses.cover_art_archive.status,
        "theaudiodb" => statuses.theaudiodb.status,
        "wikidata" => statuses.wikidata.status,
        "lastfm" => statuses.lastfm.status,
        _ => return Err(ApiError::not_found("unknown provider")),
    };
    if status == ProviderStatus::MissingApiKey {
        return Err(ApiError::forbidden("provider API key is missing"));
    }
    if status == ProviderStatus::Disabled {
        return Err(ApiError::conflict("provider is disabled"));
    }
    Ok(Json(serde_json::json!({
        "ok": true,
        "provider": provider,
        "status": status,
        "message": "Provider configuration is usable"
    })))
}
