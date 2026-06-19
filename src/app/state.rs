use crate::{config::Config, jobs, types::WorkflowPhase};
use chrono::Utc;
use serde::Serialize;
use sqlx::SqlitePool;
use std::{collections::HashSet, sync::Arc};
use tokio::sync::{RwLock, Semaphore, broadcast};

#[derive(Clone, Default, Serialize)]
pub struct Workflow {
    pub phase: WorkflowPhase,
    pub message: String,
    pub current_file: Option<String>,
    pub current: usize,
    pub total: usize,
    pub processed: usize,
    pub matched: usize,
    pub unmatched: usize,
    pub failed: usize,
    pub terminal_log: Vec<TerminalLine>,
    #[serde(skip)]
    pub cancelled: bool,
}

#[derive(Clone, Default, Serialize)]
pub struct TerminalLine {
    pub timestamp: String,
    pub level: String,
    pub stage: String,
    pub file: Option<String>,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub attempt: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub duration_ms: Option<i64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub context: Option<serde_json::Value>,
}

#[derive(Clone, Default)]
pub struct TerminalEntry {
    pub level: String,
    pub stage: String,
    pub file: Option<String>,
    pub message: String,
    pub detail: Option<String>,
    pub error: Option<String>,
    pub attempt: Option<i64>,
    pub duration_ms: Option<i64>,
    pub context: Option<serde_json::Value>,
}

impl TerminalEntry {
    pub fn new(level: &str, stage: &str, message: impl Into<String>) -> Self {
        Self {
            level: level.into(),
            stage: stage.into(),
            message: message.into(),
            ..Default::default()
        }
    }

    pub fn file(mut self, file: impl Into<String>) -> Self {
        self.file = Some(file.into());
        self
    }

    pub fn detail(mut self, detail: impl Into<String>) -> Self {
        self.detail = Some(detail.into());
        self
    }

    pub fn error(mut self, error: &(dyn std::error::Error + 'static)) -> Self {
        self.error = Some(format!("{error:#}"));
        self
    }

    pub fn error_text(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }

    pub fn attempt(mut self, attempt: i64) -> Self {
        self.attempt = Some(attempt);
        self
    }

    pub fn duration_ms(mut self, duration_ms: i64) -> Self {
        self.duration_ms = Some(duration_ms);
        self
    }

    pub fn context(mut self, context: serde_json::Value) -> Self {
        self.context = Some(context);
        self
    }
}

pub struct AppState {
    pub config: RwLock<Config>,
    pub pool: SqlitePool,
    pub client: reqwest::Client,
    pub events: broadcast::Sender<jobs::Event>,
    pub cancelled: RwLock<HashSet<String>>,
    pub artwork_downloads: RwLock<Arc<Semaphore>>,
    pub tag_writes: RwLock<Arc<Semaphore>>,
    pub workflow: RwLock<Workflow>,
}

impl AppState {
    pub fn new(config: Config, pool: SqlitePool) -> Self {
        let (events, _) = broadcast::channel(256);
        let artwork_download_concurrency = config.artwork_download_concurrency;
        let tag_write_concurrency = config.tag_write_concurrency;
        Self {
            config: RwLock::new(config),
            pool,
            client: reqwest::Client::new(),
            events,
            cancelled: Default::default(),
            artwork_downloads: RwLock::new(Arc::new(Semaphore::new(artwork_download_concurrency))),
            tag_writes: RwLock::new(Arc::new(Semaphore::new(tag_write_concurrency))),
            workflow: RwLock::new(Workflow {
                phase: WorkflowPhase::Idle,
                message: "Ready to scan".into(),
                ..Default::default()
            }),
        }
    }

    pub async fn cancelled(&self, id: &str) -> bool {
        self.cancelled.read().await.contains(id)
    }

    pub async fn refresh_limiters(&self, config: &Config) {
        *self.artwork_downloads.write().await =
            Arc::new(Semaphore::new(config.artwork_download_concurrency));
        *self.tag_writes.write().await = Arc::new(Semaphore::new(config.tag_write_concurrency));
    }

    pub async fn terminal(&self, level: &str, stage: &str, file: Option<&str>, message: &str) {
        let mut entry = TerminalEntry::new(level, stage, message);
        entry.file = file.map(str::to_owned);
        self.terminal_entry(entry).await;
    }

    pub async fn terminal_entry(&self, entry: TerminalEntry) {
        let line = TerminalLine {
            timestamp: Utc::now().to_rfc3339(),
            level: entry.level,
            stage: entry.stage,
            file: entry.file,
            message: entry.message,
            detail: entry.detail,
            error: entry.error,
            attempt: entry.attempt,
            duration_ms: entry.duration_ms,
            context: entry.context,
        };
        let mut workflow = self.workflow.write().await;
        workflow.terminal_log.push(line.clone());
        let overflow = workflow.terminal_log.len().saturating_sub(500);
        if overflow > 0 {
            workflow.terminal_log.drain(0..overflow);
        }
        let phase = workflow.phase;
        let current_file = workflow.current_file.clone();
        let current = workflow.current as i64;
        let total = workflow.total as i64;
        let processed = workflow.processed as i64;
        let matched = workflow.matched as i64;
        let unmatched = workflow.unmatched as i64;
        let failed = workflow.failed as i64;
        drop(workflow);
        let _ = self.events.send(jobs::Event {
            kind: "terminal".into(),
            stage: Some(line.stage.clone()),
            level: Some(line.level),
            file: line.file,
            timestamp: Some(line.timestamp),
            detail: line.detail,
            error: line.error,
            attempt: line.attempt,
            duration_ms: line.duration_ms,
            context: line.context,
            phase: Some(phase),
            current_file,
            processed: Some(processed),
            matched: Some(matched),
            unmatched: Some(unmatched),
            failed: Some(failed),
            current,
            total,
            message: line.message,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_line_serializes_structured_debug_fields() {
        let line = TerminalLine {
            timestamp: "2026-06-19T12:00:00Z".into(),
            level: "error".into(),
            stage: "fingerprint".into(),
            file: Some("track.flac".into()),
            message: "Fingerprint failed".into(),
            detail: Some("Running fpcalc -json".into()),
            error: Some("fpcalc failed: command not found".into()),
            attempt: Some(2),
            duration_ms: Some(1234),
            context: Some(serde_json::json!({"path": "/music/input/track.flac"})),
        };

        let value = serde_json::to_value(line).unwrap();

        assert_eq!(value["detail"], "Running fpcalc -json");
        assert_eq!(value["error"], "fpcalc failed: command not found");
        assert_eq!(value["attempt"], 2);
        assert_eq!(value["duration_ms"], 1234);
        assert_eq!(value["context"]["path"], "/music/input/track.flac");
    }
}
