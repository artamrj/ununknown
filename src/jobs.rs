use crate::api::AppState;
use serde::Serialize;
use std::sync::Arc;

#[derive(Clone, Serialize)]
pub struct Event {
    pub kind: String,
    pub stage: Option<String>,
    pub current: i64,
    pub total: i64,
    pub message: String,
}

pub fn emit(
    state: &Arc<AppState>,
    kind: &str,
    stage: Option<&str>,
    current: i64,
    total: i64,
    message: &str,
) {
    let _ = state.events.send(Event {
        kind: kind.into(),
        stage: stage.map(str::to_owned),
        current,
        total,
        message: message.into(),
    });
}
