use crate::{config::Config, types::WorkflowPhase};
use serde::Serialize;
use sqlx::SqlitePool;
use std::{sync::Arc, time::Duration};
use tokio::sync::{RwLock, Semaphore};

#[derive(Clone, Debug, Default, Serialize)]
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
    #[serde(skip)]
    pub cancelled: bool,
}

pub struct ActivityLogEntry {
    level: String,
    stage: String,
    file: Option<String>,
    message: String,
    detail: Option<String>,
    error: Option<String>,
}

impl ActivityLogEntry {
    pub fn new(
        level: impl Into<String>,
        stage: impl Into<String>,
        message: impl Into<String>,
    ) -> Self {
        Self {
            level: level.into(),
            stage: stage.into(),
            file: None,
            message: message.into(),
            detail: None,
            error: None,
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

    pub fn error(mut self, error: &dyn std::error::Error) -> Self {
        self.error = Some(error.to_string());
        self
    }

    pub fn error_text(mut self, error: impl Into<String>) -> Self {
        self.error = Some(error.into());
        self
    }

    pub fn attempt(self, _attempt: i64) -> Self {
        self
    }

    pub fn duration_ms(self, _duration_ms: i64) -> Self {
        self
    }

    pub fn context(self, _context: serde_json::Value) -> Self {
        self
    }
}

pub struct AppState {
    pub config: RwLock<Config>,
    pub pool: SqlitePool,
    pub client: reqwest::Client,
    pub artwork_downloads: RwLock<Arc<Semaphore>>,
    pub tag_writes: RwLock<Arc<Semaphore>>,
    pub workflow: RwLock<Workflow>,
}

impl AppState {
    pub fn new(config: Config, pool: SqlitePool) -> Self {
        let lookup_workers = config.lookup_workers;
        let write_workers = config.write_workers;
        Self {
            config: RwLock::new(config),
            pool,
            client: reqwest::Client::builder()
                .timeout(Duration::from_secs(12))
                .user_agent("Ununknown/0.6.0")
                .build()
                .expect("HTTP client should build"),
            artwork_downloads: RwLock::new(Arc::new(Semaphore::new(lookup_workers))),
            tag_writes: RwLock::new(Arc::new(Semaphore::new(write_workers))),
            workflow: RwLock::new(Workflow {
                message: "Ready".into(),
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
        workflow.message = "Writing corrected copies".into();
        workflow.current = 0;
        workflow.cancelled = false;
    }

    pub async fn finish_workflow(
        &self,
        phase: WorkflowPhase,
        _stage: &'static str,
        message: impl Into<String>,
    ) {
        let mut workflow = self.workflow.write().await;
        workflow.phase = phase;
        workflow.message = message.into();
        workflow.cancelled = false;
        workflow.current_file = None;
    }

    pub async fn set_workflow(
        &self,
        phase: WorkflowPhase,
        _stage: &'static str,
        message: impl Into<String>,
        current: usize,
        total: usize,
        file: Option<String>,
    ) {
        let mut workflow = self.workflow.write().await;
        workflow.phase = phase;
        workflow.message = message.into();
        workflow.current = current;
        workflow.total = total;
        workflow.current_file = file;
    }

    pub async fn start_track(
        &self,
        total: usize,
        file: impl Into<String>,
        message: impl Into<String>,
    ) {
        let mut workflow = self.workflow.write().await;
        workflow.phase = WorkflowPhase::Fetch;
        workflow.message = message.into();
        workflow.total = total;
        workflow.current_file = Some(file.into());
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

    pub async fn log(&self, level: &str, stage: &str, file: Option<&str>, message: &str) {
        let mut entry = ActivityLogEntry::new(level, stage, message);
        entry.file = file.map(str::to_owned);
        self.log_entry(entry).await;
    }

    pub async fn log_entry(&self, entry: ActivityLogEntry) {
        tracing::info!(
            level = entry.level,
            stage = entry.stage,
            file = entry.file,
            detail = entry.detail,
            error = entry.error,
            "{}",
            entry.message
        );
    }
}
