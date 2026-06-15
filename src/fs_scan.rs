use crate::{api::AppState, audio, fingerprint, jobs, providers};
use anyhow::{Result, anyhow};
use chrono::Utc;
use futures::{StreamExt, stream};
use std::{path::PathBuf, sync::Arc};
use walkdir::WalkDir;

pub async fn run(state: Arc<AppState>, job_id: String) -> Result<()> {
    let cfg = state.config.read().await.clone();
    let files: Vec<PathBuf> = WalkDir::new(&cfg.input_dir)
        .follow_links(false)
        .into_iter()
        .filter_map(Result::ok)
        .filter(|e| e.file_type().is_file() && audio::is_supported(e.path()))
        .map(|e| e.into_path())
        .collect();
    let total = files.len() as i64;
    sqlx::query("UPDATE tracks SET is_missing=1,status='missing'")
        .execute(&state.pool)
        .await?;
    let results = stream::iter(files.into_iter().enumerate().map(|(index, path)| {
        let state = state.clone();
        let job_id = job_id.clone();
        async move {
            if state.cancelled(&job_id).await {
                return Ok(());
            }
            let result = process(&state, path).await;
            jobs::progress(
                &state,
                "scan",
                &job_id,
                index as i64 + 1,
                total,
                result.as_ref().map(|_| "scanned").unwrap_or("failed"),
            )
            .await;
            result
        }
    }))
    .buffer_unordered(4)
    .collect::<Vec<Result<()>>>()
    .await;
    if let Some(error) = results.into_iter().find_map(Result::err) {
        tracing::warn!("scan item failed: {error:#}");
    }
    Ok(())
}

async fn process(state: &Arc<AppState>, path: PathBuf) -> Result<()> {
    let path_text = path.canonicalize()?.to_string_lossy().to_string();
    let metadata = tokio::fs::metadata(&path).await?;
    let info = tokio::task::spawn_blocking({
        let path = path.clone();
        move || audio::read(&path)
    })
    .await??;
    let now = Utc::now().to_rfc3339();
    let filename = path
        .file_name()
        .and_then(|v| v.to_str())
        .ok_or_else(|| anyhow!("invalid filename"))?;
    sqlx::query("INSERT INTO tracks(path,filename,format,bitrate,duration,current_title,current_artist,current_album,current_album_artist,current_track_number,file_mtime,file_size,status,is_missing,first_seen_at,last_seen_at,last_scanned_at) VALUES(?,?,?,?,?,?,?,?,?,?,?,?, 'scanned',0,?,?,?) ON CONFLICT(path) DO UPDATE SET filename=excluded.filename,format=excluded.format,bitrate=excluded.bitrate,duration=excluded.duration,current_title=excluded.current_title,current_artist=excluded.current_artist,current_album=excluded.current_album,current_album_artist=excluded.current_album_artist,current_track_number=excluded.current_track_number,file_mtime=excluded.file_mtime,file_size=excluded.file_size,status='scanned',error=NULL,is_missing=0,last_seen_at=excluded.last_seen_at,last_scanned_at=excluded.last_scanned_at")
        .bind(&path_text).bind(filename).bind(&info.format).bind(info.bitrate.map(i64::from)).bind(info.duration)
        .bind(&info.title).bind(&info.artist).bind(&info.album).bind(&info.album_artist).bind(info.track_number.map(i64::from))
        .bind(metadata.modified()?.duration_since(std::time::UNIX_EPOCH)?.as_secs() as i64).bind(metadata.len() as i64)
        .bind(&now).bind(&now).bind(&now).execute(&state.pool).await?;
    let track_id: i64 = sqlx::query_scalar("SELECT id FROM tracks WHERE path=?")
        .bind(&path_text)
        .fetch_one(&state.pool)
        .await?;
    sqlx::query("DELETE FROM candidates WHERE track_id=?")
        .bind(track_id)
        .execute(&state.pool)
        .await?;
    let (fp, duration) = match fingerprint::calculate(&path).await {
        Ok(v) => v,
        Err(e) => {
            sqlx::query("UPDATE tracks SET error=? WHERE id=?")
                .bind(e.to_string())
                .bind(track_id)
                .execute(&state.pool)
                .await?;
            return Ok(());
        }
    };
    sqlx::query("UPDATE tracks SET content_fingerprint=? WHERE id=?")
        .bind(&fp)
        .bind(track_id)
        .execute(&state.pool)
        .await?;
    let cfg = state.config.read().await.clone();
    let candidates = providers::identify(&state.client, &cfg, &fp, duration, &info)
        .await
        .unwrap_or_default();
    for c in candidates {
        let id = sqlx::query("INSERT INTO candidates(track_id,provider,title,artist,album,album_artist,track_number,track_total,disc_number,disc_total,year,genre,composer,label,isrc,cover_url,musicbrainz_recording_id,musicbrainz_release_id,musicbrainz_artist_id,musicbrainz_album_artist_id,score,raw_json) VALUES(?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?,?)")
            .bind(track_id).bind("musicbrainz").bind(c.title).bind(c.artist).bind(c.album).bind(c.album_artist)
            .bind(c.track_number).bind(c.track_total).bind(c.disc_number).bind(c.disc_total).bind(c.year).bind(c.genre)
            .bind(c.composer).bind(c.label).bind(c.isrc).bind(c.cover_url).bind(c.recording_id).bind(c.release_id)
            .bind(c.artist_id).bind(c.album_artist_id).bind(c.score).bind(c.raw_json).execute(&state.pool).await?.last_insert_rowid();
        let threshold = match cfg.automation_mode.as_str() {
            "aggressive" => 75.0,
            "manual" => 101.0,
            _ => 90.0,
        };
        if sqlx::query_scalar::<_, f64>("SELECT score FROM candidates WHERE id=?")
            .bind(id)
            .fetch_one(&state.pool)
            .await?
            >= threshold
        {
            sqlx::query("UPDATE tracks SET selected_candidate_id=?,status='selected' WHERE id=? AND selected_candidate_id IS NULL").bind(id).bind(track_id).execute(&state.pool).await?;
        }
    }
    sqlx::query("UPDATE tracks SET status=CASE WHEN selected_candidate_id IS NOT NULL THEN 'selected' WHEN EXISTS(SELECT 1 FROM candidates WHERE track_id=tracks.id) THEN 'needs_review' ELSE 'no_match' END WHERE id=?").bind(track_id).execute(&state.pool).await?;
    Ok(())
}
