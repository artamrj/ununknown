use crate::api::AppState;
use anyhow::Result;
use chrono::Utc;
use serde::Serialize;
use std::sync::Arc;
use uuid::Uuid;

#[derive(Clone, Serialize)]
pub struct Event {
    pub kind: String,
    pub job_id: String,
    pub track_id: Option<i64>,
    pub stage: Option<String>,
    pub current: i64,
    pub total: i64,
    pub message: String,
}

pub async fn create(state: &Arc<AppState>, kind: &str) -> Result<String> {
    let id = Uuid::new_v4().to_string();
    let now = Utc::now().to_rfc3339();
    sqlx::query(
        "INSERT INTO jobs(id,kind,status,created_at,updated_at) VALUES(?,?, 'running',?,?)",
    )
    .bind(&id)
    .bind(kind)
    .bind(&now)
    .bind(&now)
    .execute(&state.pool)
    .await?;
    emit(state, kind, &id, 0, 0, "started");
    Ok(id)
}
pub async fn progress(
    state: &Arc<AppState>,
    kind: &str,
    id: &str,
    current: i64,
    total: i64,
    message: &str,
) {
    let _ =
        sqlx::query("UPDATE jobs SET progress_current=?,progress_total=?,updated_at=? WHERE id=?")
            .bind(current)
            .bind(total)
            .bind(Utc::now().to_rfc3339())
            .bind(id)
            .execute(&state.pool)
            .await;
    emit(state, kind, id, current, total, message);
}
pub async fn finish(state: &Arc<AppState>, kind: &str, id: &str, error: Option<&str>) {
    let status = if error.is_some() {
        "failed"
    } else {
        "completed"
    };
    let _ = sqlx::query("UPDATE jobs SET status=?,error=?,updated_at=? WHERE id=?")
        .bind(status)
        .bind(error)
        .bind(Utc::now().to_rfc3339())
        .bind(id)
        .execute(&state.pool)
        .await;
    emit(state, kind, id, 0, 0, status);
}
pub fn track(state: &Arc<AppState>, job_id: &str, track_id: i64, stage: &str, message: &str) {
    let _ = state.events.send(Event {
        kind: "track".into(),
        job_id: job_id.into(),
        track_id: Some(track_id),
        stage: Some(stage.into()),
        current: 0,
        total: 0,
        message: message.into(),
    });
}
fn emit(state: &Arc<AppState>, kind: &str, id: &str, current: i64, total: i64, message: &str) {
    let _ = state.events.send(Event {
        kind: kind.into(),
        job_id: id.into(),
        track_id: None,
        stage: None,
        current,
        total,
        message: message.into(),
    });
}
