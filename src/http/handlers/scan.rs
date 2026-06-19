use super::*;

pub async fn start_scan(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    if s.workflow_running().await {
        return Err(ApiError::conflict("workflow is already running"));
    }
    sqlx::query("DELETE FROM tracks").execute(&s.pool).await?;
    previews::invalidate(&s.pool).await?;
    s.reset_workflow(WorkflowPhase::Scan, "Discovering music")
        .await;
    s.terminal(
        "info",
        "scan",
        None,
        "Starting new scan; cleared previous temporary workspace",
    )
    .await;
    let state = s.clone();
    tokio::spawn(async move {
        if let Err(error) = scan_pipeline::run(state.clone()).await {
            state
                .finish_workflow(WorkflowPhase::Failed, "failed", error.to_string())
                .await;
        }
    });
    Ok(Json(serde_json::json!({"started":true})))
}
pub async fn stop_scan(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    s.cancel_workflow().await;
    Ok(Json(serde_json::json!({"stopping":true})))
}
pub async fn list_jobs(State(s): State<Arc<AppState>>) -> ApiResult<Json<Vec<serde_json::Value>>> {
    Ok(Json(vec![serde_json::to_value(
        s.workflow.read().await.clone(),
    )?]))
}
pub async fn get_job(
    State(s): State<Arc<AppState>>,
    Path(id): Path<String>,
) -> ApiResult<Json<serde_json::Value>> {
    let _ = id;
    Ok(Json(serde_json::to_value(s.workflow.read().await.clone())?))
}
