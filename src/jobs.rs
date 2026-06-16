use crate::api::AppState;
use serde::Serialize;
use std::sync::Arc;

#[derive(Clone, Serialize)]
pub struct Event {
    pub kind: String,
    pub stage: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub level: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub timestamp: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub phase: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_file: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub processed: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub matched: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unmatched: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failed: Option<i64>,
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
        level: None,
        file: None,
        timestamp: None,
        phase: stage.map(str::to_owned),
        current_file: None,
        processed: None,
        matched: None,
        unmatched: None,
        failed: None,
        current,
        total,
        message: message.into(),
    });
}
