use super::*;

pub async fn workspace(State(s): State<Arc<AppState>>) -> ApiResult<Json<serde_json::Value>> {
    let mut workflow = s.workflow.read().await.clone();
    workflow.matched = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM tracks WHERE selected_candidate_id IS NOT NULL",
    )
    .fetch_one(&s.pool)
    .await? as usize;
    workflow.unmatched = sqlx::query_scalar::<_, i64>(
        "SELECT count(*) FROM tracks WHERE stage='review' AND selected_candidate_id IS NULL",
    )
    .fetch_one(&s.pool)
    .await? as usize;
    workflow.failed =
        sqlx::query_scalar::<_, i64>("SELECT count(*) FROM tracks WHERE stage='failed'")
            .fetch_one(&s.pool)
            .await? as usize;
    Ok(Json(serde_json::to_value(workflow)?))
}
