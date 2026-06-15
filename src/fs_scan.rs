use crate::{api::AppState, audio, fingerprint, jobs, providers};
use anyhow::{Result, anyhow};
use chrono::Utc;
use std::{
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};
use walkdir::WalkDir;

pub async fn run(state: Arc<AppState>) -> Result<()> {
    let cfg = state.config.read().await.clone();
    set_phase(&state, "scan", "Discovering music", 0, 0, None).await;
    let mut files: Vec<PathBuf> = WalkDir::new(&cfg.input_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && audio::is_supported(e.path()))
        .filter_map(|e| e.path().canonicalize().ok())
        .collect();
    files.sort();
    let total = files.len();
    {
        let mut w = state.workflow.write().await;
        w.total = total;
        w.phase = "fetch".into();
        w.message = "Starting sequential matching".into();
    }
    jobs::emit(
        &state,
        "workflow",
        Some("fetch"),
        0,
        total as i64,
        "Starting sequential matching",
    );

    for (index, path) in files.into_iter().enumerate() {
        if state.workflow.read().await.cancelled {
            break;
        }
        let filename = path
            .file_name()
            .and_then(|v| v.to_str())
            .unwrap_or("audio")
            .to_owned();
        for attempt in 1..=cfg.track_attempts {
            set_phase(
                &state,
                "fetch",
                &format!(
                    "Matching {filename} · attempt {attempt}/{}",
                    cfg.track_attempts
                ),
                index,
                total,
                Some(filename.clone()),
            )
            .await;
            match process(&state, &path).await {
                Ok(true) => {
                    break;
                }
                Ok(false) => break,
                Err(error) if attempt < cfg.track_attempts => {
                    tracing::warn!(path=%path.display(), attempt, "track attempt failed: {error:#}");
                    tokio::time::sleep(Duration::from_millis(400 * attempt as u64)).await;
                }
                Err(error) => {
                    tracing::warn!(path=%path.display(), "track failed after retries: {error:#}");
                    state.workflow.write().await.failed += 1;
                }
            }
        }
        state.workflow.write().await.processed = index + 1;
    }
    let cancelled = state.workflow.read().await.cancelled;
    set_phase(
        &state,
        if cancelled { "idle" } else { "preview" },
        if cancelled {
            "Scan stopped"
        } else {
            "Matching complete"
        },
        total,
        total,
        None,
    )
    .await;
    Ok(())
}

async fn process(state: &Arc<AppState>, path: &Path) -> Result<bool> {
    let info = tokio::task::spawn_blocking({
        let p = path.to_path_buf();
        move || audio::read(&p)
    })
    .await??;
    let (fp, duration) = fingerprint::calculate(path).await?;
    let cfg = state.config.read().await.clone();
    let candidates = providers::identify(&state.client, &cfg, &fp, duration, &info).await?;
    let threshold = match cfg.automation_mode.as_str() {
        "aggressive" => 75.0,
        "manual" => 101.0,
        "custom" => cfg.confidence_threshold,
        _ => 90.0,
    };
    let Some(candidate) = candidates
        .into_iter()
        .max_by(|a, b| a.score.total_cmp(&b.score))
        .filter(|c| c.score >= threshold)
    else {
        state.workflow.write().await.unmatched += 1;
        return Ok(false);
    };
    persist_match(state, path, &info, &candidate).await?;
    state.workflow.write().await.matched += 1;
    Ok(true)
}

async fn persist_match(
    state: &Arc<AppState>,
    path: &Path,
    info: &audio::AudioInfo,
    c: &providers::Candidate,
) -> Result<()> {
    let now = Utc::now().to_rfc3339();
    let text = path.to_string_lossy();
    let filename = path
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or_else(|| anyhow!("invalid filename"))?;
    let id=sqlx::query("INSERT INTO tracks(path,filename,format,duration,current_title,current_artist,current_album,status,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage,updated_at) VALUES(?,?,?,?,?,?,?,'selected',0,?,?,?,'ready',?)")
        .bind(text.as_ref()).bind(filename).bind(&info.format).bind(info.duration).bind(&info.title).bind(&info.artist).bind(&info.album).bind(&now).bind(&now).bind(&now).bind(&now).execute(&state.pool).await?.last_insert_rowid();
    let cid=sqlx::query("INSERT INTO candidates(track_id,provider,title,artist,album,album_artist,track_number,track_total,disc_number,disc_total,year,genre,composer,label,isrc,cover_url,musicbrainz_recording_id,musicbrainz_release_id,musicbrainz_artist_id,musicbrainz_album_artist_id,score,raw_json) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
        .bind(id).bind("musicbrainz").bind(&c.title).bind(&c.artist).bind(&c.album).bind(&c.album_artist).bind(c.track_number).bind(c.track_total).bind(c.disc_number).bind(c.disc_total).bind(&c.year).bind(&c.genre).bind(&c.composer).bind(&c.label).bind(&c.isrc).bind(&c.cover_url).bind(&c.recording_id).bind(&c.release_id).bind(&c.artist_id).bind(&c.album_artist_id).bind(c.score).bind(&c.raw_json).execute(&state.pool).await?.last_insert_rowid();
    sqlx::query("UPDATE tracks SET selected_candidate_id=? WHERE id=?")
        .bind(cid)
        .bind(id)
        .execute(&state.pool)
        .await?;
    Ok(())
}

async fn set_phase(
    state: &Arc<AppState>,
    phase: &str,
    message: &str,
    current: usize,
    total: usize,
    file: Option<String>,
) {
    let mut w = state.workflow.write().await;
    w.phase = phase.into();
    w.message = message.into();
    w.current = current;
    w.total = total;
    w.current_file = file;
    drop(w);
    jobs::emit(
        state,
        "workflow",
        Some(phase),
        current as i64,
        total as i64,
        message,
    );
}
