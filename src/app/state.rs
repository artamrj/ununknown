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
        let line = TerminalLine {
            timestamp: Utc::now().to_rfc3339(),
            level: level.into(),
            stage: stage.into(),
            file: file.map(str::to_owned),
            message: message.into(),
        };
        let mut workflow = self.workflow.write().await;
        workflow.terminal_log.push(line.clone());
        let overflow = workflow.terminal_log.len().saturating_sub(160);
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
            stage: Some(stage.into()),
            level: Some(line.level),
            file: line.file,
            timestamp: Some(line.timestamp),
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
