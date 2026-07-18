use super::*;
use crate::app::ActivityLogEntry;

#[derive(Serialize)]
pub struct RetryIssuesResult {
    started: bool,
    queued: usize,
    unavailable: usize,
}

pub async fn start_scan(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    if s.workflow_running().await {
        return Err(ApiError::conflict("identification is already running"));
    }
    sqlx::query("DELETE FROM tracks").execute(&s.pool).await?;
    s.reset_workflow(WorkflowPhase::Scan, "Discovering music")
        .await;
    let state = s.clone();
    tokio::spawn(async move {
        if let Err(error) = scan_pipeline::run(state.clone()).await {
            state
                .finish_workflow(WorkflowPhase::Failed, "failed", error.to_string())
                .await;
        }
    });
    Ok(Json(serde_json::json!({"started": true})))
}

pub async fn stop_scan(State(s): State<Arc<AppState>>) -> Json<serde_json::Value> {
    s.cancel_workflow().await;
    Json(serde_json::json!({"stopping": true}))
}

pub async fn retry_issues(State(s): State<Arc<AppState>>) -> ApiResult<Json<RetryIssuesResult>> {
    if s.workflow_running().await {
        return Err(ApiError::conflict(
            "Wait for the current scan or write operation to finish",
        ));
    }

    let issue_paths: Vec<(i64, String, String)> = sqlx::query_as(
        "SELECT id,path,status FROM tracks
         WHERE status IN ('corrupt','failed','provider_error')
            OR is_missing=1 OR stage='failed'
         ORDER BY path",
    )
    .fetch_all(&s.pool)
    .await?;

    let mut available = Vec::new();
    let mut unavailable = 0_usize;
    for (id, path_text, status) in issue_paths {
        let path = PathBuf::from(&path_text);
        if tokio::fs::metadata(&path)
            .await
            .is_ok_and(|metadata| metadata.is_file())
        {
            sqlx::query(
                "UPDATE tracks SET stage_message='Checking and repairing this file',
                 retry_count=retry_count+1,next_retry_at=NULL,updated_at=? WHERE id=?",
            )
            .bind(chrono::Utc::now().to_rfc3339())
            .bind(id)
            .execute(&s.pool)
            .await?;
            available.push((path, status == "corrupt"));
        } else {
            unavailable += 1;
            sqlx::query(
                "UPDATE tracks SET status='failed',stage='failed',is_missing=1,
                 stage_message='Source file is still missing; restore it to its original location and check again',
                 retry_count=retry_count+1,next_retry_at=NULL,updated_at=? WHERE id=?",
            )
            .bind(chrono::Utc::now().to_rfc3339())
            .bind(id)
            .execute(&s.pool)
            .await?;
        }
    }

    let queued = available.len();
    if queued == 0 {
        return Ok(Json(RetryIssuesResult {
            started: false,
            queued,
            unavailable,
        }));
    }

    s.reset_workflow(WorkflowPhase::Fetch, "Checking files with issues")
        .await;
    let state = s.clone();
    tokio::spawn(async move {
        if let Err(error) = recover_issue_files(state.clone(), available).await {
            state
                .finish_workflow(WorkflowPhase::Failed, "failed", error.to_string())
                .await;
        }
    });
    Ok(Json(RetryIssuesResult {
        started: true,
        queued,
        unavailable,
    }))
}

async fn recover_issue_files(state: Arc<AppState>, issues: Vec<(PathBuf, bool)>) -> Result<()> {
    let total = issues.len();
    let mut retry_paths = Vec::with_capacity(total);
    for (index, (path, damaged)) in issues.into_iter().enumerate() {
        if state.workflow_cancelled().await {
            break;
        }
        let filename = path
            .file_name()
            .and_then(|value| value.to_str())
            .unwrap_or("audio")
            .to_owned();
        if damaged {
            state
                .set_workflow(
                    WorkflowPhase::Fetch,
                    "repair",
                    "Salvaging damaged audio",
                    index,
                    total,
                    Some(filename.clone()),
                )
                .await;
            match crate::infrastructure::media::repair::repair(&state.pool, &path).await {
                Ok(repair) => {
                    let message = format!(
                        "Repaired {:.1}s of {:.1}s; damaged backup saved as {}",
                        repair.repaired_duration,
                        repair.original_duration,
                        repair.backup_path.display()
                    );
                    sqlx::query(
                        "UPDATE tracks SET status='processing',stage='discovered',error=NULL,
                         is_missing=0,stage_message=?,updated_at=? WHERE path=?",
                    )
                    .bind(&message)
                    .bind(chrono::Utc::now().to_rfc3339())
                    .bind(path.to_string_lossy().as_ref())
                    .execute(&state.pool)
                    .await?;
                    state.log("ok", "repair", Some(&filename), &message).await;
                    retry_paths.push(path);
                }
                Err(error) => {
                    let detail = format!("{error:#}");
                    sqlx::query(
                        "UPDATE tracks SET status='corrupt',stage='failed',
                         stage_message='Automatic repair could not recover enough valid audio; the original was left unchanged',
                         error=?,updated_at=? WHERE path=?",
                    )
                    .bind(&detail)
                    .bind(chrono::Utc::now().to_rfc3339())
                    .bind(path.to_string_lossy().as_ref())
                    .execute(&state.pool)
                    .await?;
                    state.increment_failed().await;
                    state
                        .log_entry(
                            ActivityLogEntry::new(
                                "error",
                                "repair",
                                "Automatic audio repair failed; original preserved",
                            )
                            .file(filename)
                            .error_text(detail),
                        )
                        .await;
                }
            }
        } else {
            // An explicit retry must decode the audio again, even when the
            // unchanged file previously produced a cached integrity result.
            sqlx::query("DELETE FROM integrity_cache WHERE path=?")
                .bind(path.to_string_lossy().as_ref())
                .execute(&state.pool)
                .await?;
            retry_paths.push(path);
        }
    }

    if retry_paths.is_empty() {
        state
            .finish_workflow(
                WorkflowPhase::Preview,
                "preview",
                "Issue check complete; damaged originals were preserved",
            )
            .await;
        return Ok(());
    }
    scan_pipeline::retry_files(state, retry_paths).await
}

#[cfg(test)]
mod tests {
    use super::*;

    async fn test_state() -> Arc<AppState> {
        let directory = tempfile::tempdir().unwrap();
        let database = directory.path().join("retry-issues.sqlite");
        let pool = crate::infrastructure::db::connect(database.to_str().unwrap())
            .await
            .unwrap();
        std::mem::forget(directory);
        Arc::new(AppState::new(Config::default(), pool))
    }

    #[tokio::test]
    async fn retry_issues_keeps_missing_files_actionable() {
        let state = test_state().await;
        sqlx::query(
            "INSERT INTO tracks(path,filename,status,is_missing,first_seen_at,last_seen_at,last_scanned_at,stage)
             VALUES('/music/missing.mp3','missing.mp3','failed',1,'now','now','now','failed')",
        )
        .execute(&state.pool)
        .await
        .unwrap();

        let Json(result) = retry_issues(State(state.clone())).await.unwrap();

        assert!(!result.started);
        assert_eq!(result.queued, 0);
        assert_eq!(result.unavailable, 1);
        let row: (String, String, bool, i64) = sqlx::query_as(
            "SELECT status,stage_message,is_missing,retry_count FROM tracks WHERE filename='missing.mp3'",
        )
        .fetch_one(&state.pool)
        .await
        .unwrap();
        assert_eq!(row.0, "failed");
        assert!(row.1.contains("still missing"));
        assert!(row.2);
        assert_eq!(row.3, 1);
    }
}
