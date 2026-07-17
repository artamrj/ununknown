use super::*;

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
