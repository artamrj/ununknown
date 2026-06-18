use super::*;

pub async fn start_scan(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    if matches!(
        s.workflow.read().await.phase,
        WorkflowPhase::Scan | WorkflowPhase::Fetch | WorkflowPhase::Apply
    ) {
        return Err(anyhow!("workflow is already running").into());
    }
    sqlx::query("DELETE FROM tracks").execute(&s.pool).await?;
    sqlx::query("DELETE FROM provider_cache")
        .execute(&s.pool)
        .await?;
    s.previews.write().await.clear();
    *s.workflow.write().await = Workflow {
        phase: WorkflowPhase::Scan,
        message: "Discovering music".into(),
        ..Default::default()
    };
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
            let mut w = state.workflow.write().await;
            w.phase = WorkflowPhase::Failed;
            w.message = error.to_string();
            jobs::emit(
                &state,
                "workflow",
                Some("failed"),
                Some(WorkflowPhase::Failed),
                0,
                0,
                &error.to_string(),
            );
        }
    });
    Ok(Json(serde_json::json!({"started":true})))
}
pub async fn stop_scan(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    s.workflow.write().await.cancelled = true;
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
