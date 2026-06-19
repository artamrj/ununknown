use crate::{config::Config, jobs, types::WorkflowPhase};
use chrono::Utc;
use serde::Serialize;
use sqlx::SqlitePool;
use std::sync::Arc;
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
    pub activity_log: Vec<ActivityLogLine>,
    #[serde(skip)]
    pub cancelled: bool,
}

#[derive(Clone, Default, Serialize)]
pub struct ActivityLogLine {
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
pub struct ActivityLogEntry {
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

impl ActivityLogEntry {
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
            artwork_downloads: RwLock::new(Arc::new(Semaphore::new(artwork_download_concurrency))),
            tag_writes: RwLock::new(Arc::new(Semaphore::new(tag_write_concurrency))),
            workflow: RwLock::new(Workflow {
                phase: WorkflowPhase::Idle,
                message: "Ready to scan".into(),
                ..Default::default()
            }),
        }
    }

    pub async fn workflow_running(&self) -> bool {
        matches!(
            self.workflow.read().await.phase,
            WorkflowPhase::Scan | WorkflowPhase::Fetch | WorkflowPhase::Apply
        )
    }

    pub async fn reset_workflow(&self, phase: WorkflowPhase, message: impl Into<String>) {
        *self.workflow.write().await = Workflow {
            phase,
            message: message.into(),
            ..Default::default()
        };
    }

    pub async fn cancel_workflow(&self) {
        self.workflow.write().await.cancelled = true;
    }

    pub async fn workflow_cancelled(&self) -> bool {
        self.workflow.read().await.cancelled
    }

    pub async fn start_apply_workflow(&self) {
        let mut workflow = self.workflow.write().await;
        workflow.phase = WorkflowPhase::Apply;
        workflow.message = "Applying matched metadata".into();
        workflow.current = 0;
        workflow.total = 0;
        workflow.current_file = None;
        workflow.cancelled = false;
    }

    pub async fn finish_workflow(
        &self,
        phase: WorkflowPhase,
        stage: &'static str,
        message: impl Into<String>,
    ) {
        let message = message.into();
        let (current, total) = {
            let mut workflow = self.workflow.write().await;
            workflow.phase = phase;
            workflow.message = message.clone();
            workflow.cancelled = false;
            (workflow.current as i64, workflow.total as i64)
        };
        let _ = self.events.send(jobs::Event {
            kind: "workflow".into(),
            stage: Some(stage.into()),
            level: None,
            file: None,
            timestamp: None,
            detail: None,
            error: None,
            attempt: None,
            duration_ms: None,
            context: None,
            phase: Some(phase),
            current_file: None,
            processed: None,
            matched: None,
            unmatched: None,
            failed: None,
            current,
            total,
            message,
        });
    }

    pub async fn set_workflow(
        &self,
        phase: WorkflowPhase,
        stage: &'static str,
        message: impl Into<String>,
        current: usize,
        total: usize,
        file: Option<String>,
    ) {
        let message = message.into();
        let mut workflow = self.workflow.write().await;
        workflow.phase = phase;
        workflow.message = message.clone();
        workflow.current = current;
        workflow.total = total;
        workflow.current_file = file;
        drop(workflow);
        let _ = self.events.send(jobs::Event {
            kind: "workflow".into(),
            stage: Some(stage.into()),
            level: None,
            file: None,
            timestamp: None,
            detail: None,
            error: None,
            attempt: None,
            duration_ms: None,
            context: None,
            phase: Some(phase),
            current_file: None,
            processed: None,
            matched: None,
            unmatched: None,
            failed: None,
            current: current as i64,
            total: total as i64,
            message,
        });
    }

    pub async fn increment_failed(&self) {
        self.workflow.write().await.failed += 1;
    }

    pub async fn increment_matched(&self) {
        self.workflow.write().await.matched += 1;
    }

    pub async fn increment_unmatched(&self) {
        self.workflow.write().await.unmatched += 1;
    }

    pub async fn finish_track(&self, total: usize) -> usize {
        let mut workflow = self.workflow.write().await;
        workflow.processed += 1;
        workflow.current = workflow.processed;
        workflow.total = total;
        workflow.processed
    }

    pub async fn refresh_limiters(&self, config: &Config) {
        *self.artwork_downloads.write().await =
            Arc::new(Semaphore::new(config.artwork_download_concurrency));
        *self.tag_writes.write().await = Arc::new(Semaphore::new(config.tag_write_concurrency));
    }

    pub async fn log(&self, level: &str, stage: &str, file: Option<&str>, message: &str) {
        let mut entry = ActivityLogEntry::new(level, stage, message);
        entry.file = file.map(str::to_owned);
        self.log_entry(entry).await;
    }

    pub async fn log_entry(&self, entry: ActivityLogEntry) {
        let line = ActivityLogLine {
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
        workflow.activity_log.push(line.clone());
        let overflow = workflow.activity_log.len().saturating_sub(500);
        if overflow > 0 {
            workflow.activity_log.drain(0..overflow);
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
            kind: "activity_log".into(),
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
    fn activity_log_line_serializes_structured_debug_fields() {
        let line = ActivityLogLine {
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

    #[tokio::test]
    async fn workflow_controller_updates_state_and_cancellation() {
        let pool = SqlitePool::connect("sqlite::memory:").await.unwrap();
        let state = AppState::new(Config::default(), pool);

        assert!(!state.workflow_running().await);
        state
            .set_workflow(WorkflowPhase::Scan, "scan", "Scanning", 0, 10, None)
            .await;
        assert!(state.workflow_running().await);

        state.cancel_workflow().await;
        assert!(state.workflow_cancelled().await);

        state
            .finish_workflow(WorkflowPhase::Idle, "idle", "Scan stopped")
            .await;
        assert!(!state.workflow_running().await);
        assert!(!state.workflow_cancelled().await);
    }
}
