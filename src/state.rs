use crate::config::Config;
use crate::models::Router;
use crate::tools::ToolRegistry;
use anyhow::Result;
use serde::Serialize;
use sqlx::SqlitePool;
use std::collections::HashMap;
use std::sync::Arc;
use tokio::sync::{broadcast, RwLock, Semaphore};
use tokio_util::sync::CancellationToken;

#[derive(Debug, Clone, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum Event {
    Message {
        conversation_id: String,
        role: String,
        content: String,
    },
    TaskUpdate {
        task_id: String,
        status: String,
        title: String,
    },
    AgentUpdate {
        run_id: String,
        task_id: String,
        role: String,
        status: String,
        detail: String,
    },
    ToolCall {
        task_id: Option<String>,
        tool: String,
        ok: bool,
        summary: String,
    },
    ApprovalRequired {
        id: String,
        action: String,
        risk: String,
        preview: String,
    },
    Notification {
        category: String,
        message: String,
    },
}

pub struct Inner {
    pub db: SqlitePool,
    pub config: RwLock<Config>,
    pub config_sources: RwLock<crate::config::Sources>,
    pub router: RwLock<Arc<Router>>,
    pub registry: Arc<ToolRegistry>,
    pub subagent_sem: Semaphore,
    pub cancels: RwLock<HashMap<String, CancellationToken>>,
    pub events: broadcast::Sender<Event>,
    pub http: reqwest::Client,
}

#[derive(Clone)]
pub struct AppState(pub Arc<Inner>);

impl std::ops::Deref for AppState {
    type Target = Inner;
    fn deref(&self) -> &Self::Target {
        &self.0
    }
}

impl AppState {
    pub fn new(db: SqlitePool, loaded: crate::config::Loaded) -> Self {
        let (tx, _) = broadcast::channel(512);
        let router = crate::models::build_router(&loaded.config);
        let permits = loaded.config.agents.hard_max_subagents;
        AppState(Arc::new(Inner {
            db,
            subagent_sem: Semaphore::new(permits),
            config: RwLock::new(loaded.config),
            config_sources: RwLock::new(loaded.sources),
            router: RwLock::new(Arc::new(router)),
            registry: Arc::new(crate::tools::registry()),
            cancels: RwLock::new(HashMap::new()),
            events: tx,
            http: reqwest::Client::builder()
                .timeout(std::time::Duration::from_secs(30))
                .user_agent("FRIDAY/0.1 (+https://github.com/)")
                .build()
                .expect("http client"),
        }))
    }

    pub fn emit(&self, e: Event) {
        let _ = self.events.send(e);
    }

    pub async fn cfg(&self) -> Config {
        self.config.read().await.clone()
    }

    pub async fn audit(
        &self,
        actor: &str,
        category: &str,
        action: &str,
        detail: &str,
        task: Option<&str>,
    ) {
        let r = sqlx::query(
            "INSERT INTO audit_log (actor, category, action, detail, task_id, created_at) VALUES (?,?,?,?,?,?)",
        )
        .bind(actor)
        .bind(category)
        .bind(action)
        .bind(crate::util::truncate(&crate::util::redact(detail), 4000))
        .bind(task)
        .bind(crate::util::now())
        .execute(&self.db)
        .await;
        if let Err(e) = r {
            tracing::warn!("audit write failed: {e}");
        }
    }

    /// Records a Proof-Carrying Action evidence row.
    pub async fn record_evidence(
        &self,
        task_id: &str,
        claim: &str,
        kind: &str,
        tool_call_id: Option<&str>,
        detail: &str,
        verified: bool,
    ) -> Result<String> {
        let id = crate::util::uid();
        sqlx::query(
            "INSERT INTO evidence (id, task_id, claim, kind, tool_call_id, detail, verified, created_at)
             VALUES (?,?,?,?,?,?,?,?)",
        )
        .bind(&id)
        .bind(task_id)
        .bind(claim)
        .bind(kind)
        .bind(tool_call_id)
        .bind(crate::util::truncate(&crate::util::redact(detail), 8000))
        .bind(verified as i64)
        .bind(crate::util::now())
        .execute(&self.db)
        .await?;
        Ok(id)
    }

    pub async fn register_cancel(&self, task_id: &str) -> CancellationToken {
        let t = CancellationToken::new();
        self.cancels
            .write()
            .await
            .insert(task_id.to_string(), t.clone());
        t
    }

    pub async fn cancel(&self, task_id: &str) -> bool {
        let mut map = self.cancels.write().await;
        if let Some(t) = map.remove(task_id) {
            t.cancel();
            return true;
        }
        false
    }

    /// Rebuilds the model router after a configuration change.
    pub async fn reload_router(&self) {
        let cfg = self.cfg().await;
        *self.router.write().await = Arc::new(crate::models::build_router(&cfg));
    }
}
