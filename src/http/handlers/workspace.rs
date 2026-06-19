use super::*;

pub async fn clear_workspace(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    sqlx::query("DELETE FROM tracks").execute(&s.pool).await?;
    sqlx::query("DELETE FROM jobs").execute(&s.pool).await?;
    previews::invalidate(&s.pool).await?;
    s.reset_workflow(WorkflowPhase::Idle, "Ready to scan").await;
    Ok(Json(serde_json::json!({"cleared":true})))
}
pub async fn workspace(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    let mut workflow = s.workflow.read().await.clone();
    let matched: i64 =
        sqlx::query_scalar("SELECT count(*) FROM tracks WHERE selected_candidate_id IS NOT NULL")
            .fetch_one(&s.pool)
            .await?;
    workflow.matched = matched as usize;
    if workflow.phase == WorkflowPhase::Idle && matched > 0 {
        workflow.phase = WorkflowPhase::Preview;
        workflow.message = "Restored matched preview".into();
    }
    let mut value = serde_json::to_value(workflow)?;
    if let serde_json::Value::Object(fields) = &mut value {
        if let Some(activity_log) = fields.get("activity_log").cloned() {
            fields.insert("terminal_log".into(), activity_log);
        }
    }
    Ok(Json(value))
}
