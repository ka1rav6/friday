//! HTTP + WebSocket API. The frontend, the CLI and any external client all go
//! through this one surface, so GUI and CLI can never disagree.
use crate::state::AppState;
use axum::extract::{Path, Query, State, WebSocketUpgrade};
use axum::http::StatusCode;
use axum::response::{IntoResponse, Response};
use axum::routing::{delete, get, post, put};
use axum::{Json, Router};
use serde::Deserialize;
use serde_json::{json, Value};
use tower_http::cors::CorsLayer;
use tower_http::services::{ServeDir, ServeFile};

pub struct ApiError(anyhow::Error);
impl IntoResponse for ApiError {
    fn into_response(self) -> Response {
        let msg = crate::util::redact(&format!("{:#}", self.0));
        tracing::warn!("api error: {msg}");
        (StatusCode::BAD_REQUEST, Json(json!({"error": msg}))).into_response()
    }
}
impl<E: Into<anyhow::Error>> From<E> for ApiError {
    fn from(e: E) -> Self {
        ApiError(e.into())
    }
}
type ApiResult = Result<Json<Value>, ApiError>;

/// Rejects a request with an actionable message instead of a generic 400.
macro_rules! require {
    ($cond:expr, $($arg:tt)*) => {
        if !$cond {
            return Err(ApiError(anyhow::anyhow!($($arg)*)));
        }
    };
}

pub fn router(state: AppState) -> Router {
    let api = Router::new()
        .route("/api/health", get(health))
        .route("/api/status", get(status))
        .route("/api/tools", get(tools))
        .route("/api/config", get(get_config).put(put_config))
        .route("/api/config/validate", post(validate_config))
        .route("/api/config/reset", post(reset_config))
        .route("/api/chat", post(chat))
        .route("/api/conversations/:id/messages", get(messages))
        .route("/api/tasks", get(tasks))
        .route("/api/tasks/:id", get(task_detail))
        .route("/api/tasks/:id/cancel", post(cancel))
        .route("/api/agents", get(agents))
        .route("/api/memories", get(list_memories).post(create_memory))
        .route("/api/memories/conflicts", get(memory_conflicts))
        .route("/api/memories/:id", put(edit_memory).delete(remove_memory))
        .route("/api/memories/:id/approve", post(approve_memory))
        .route("/api/research", get(list_research))
        .route("/api/research/:id", get(get_research))
        .route("/api/skills", get(list_skills).post(add_skill))
        .route("/api/skills/rescan", post(rescan_skills))
        .route("/api/skills/inspect", post(inspect_skill))
        .route("/api/skills/:name", delete(remove_skill))
        .route("/api/skills/:name/enabled", put(set_skill_enabled))
        .route("/api/skills/:name/run", post(run_skill))
        .route("/api/skills/:name/runs", get(skill_runs))
        .route("/api/jobs", get(jobs))
        .route("/api/jobs/:name/run", post(run_job))
        .route("/api/briefing", get(briefing))
        .route("/api/approvals", get(approvals))
        .route("/api/approvals/:id/decide", post(decide_approval))
        .route("/api/audit", get(audit))
        .route("/api/events", get(events_ws))
        .layer(CorsLayer::permissive())
        .with_state(state);

    // Serve the built frontend from the same process when it is present, so a
    // deployment is one binary. `index.html` is the fallback, because the UI is
    // a single page with client-side tabs.
    match static_dir() {
        Some(dir) => {
            tracing::info!(dir = %dir.display(), "serving the built frontend");
            let index = dir.join("index.html");
            api.fallback_service(ServeDir::new(dir).fallback(ServeFile::new(index)))
        }
        None => {
            tracing::info!("no built frontend found; API only (run `npm run dev` in frontend/)");
            api
        }
    }
}

/// `FRIDAY_STATIC`, else `frontend/dist` if it has been built.
fn static_dir() -> Option<std::path::PathBuf> {
    let candidate = std::env::var_os("FRIDAY_STATIC")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("frontend/dist"));
    candidate.join("index.html").is_file().then_some(candidate)
}

async fn health() -> ApiResult {
    Ok(Json(
        json!({"ok": true, "service": "friday", "version": env!("CARGO_PKG_VERSION")}),
    ))
}

async fn status(State(st): State<AppState>) -> ApiResult {
    let ctx = crate::tools::ToolCtx::root(st.clone(), None);
    let v = crate::tools::execute(&ctx, "get_system_status", json!({})).await;
    Ok(Json(v.value))
}

async fn tools(State(st): State<AppState>) -> ApiResult {
    Ok(Json(json!({"tools": st.registry.defs()})))
}

// ------------------------------------------------------------------ config

async fn get_config(State(st): State<AppState>) -> ApiResult {
    let cfg = st.cfg().await;
    let sources = st.config_sources.read().await;
    Ok(Json(json!({
        "config": crate::config::redacted(&cfg),
        "sources": {
            "precedence": ["defaults", "user config", "project config", "environment", "cli"],
            "user_config": sources.user_config,
            "project_config": sources.project_config,
            "env_overrides": sources.env_overrides,
            "project_config_path": crate::config::PROJECT_CONFIG,
            "user_config_path": crate::config::dirs_config().join(crate::config::USER_CONFIG_REL).display().to_string()
        },
        "restart_required_for": ["bind", "database_url"],
        "secrets_are_never_returned": true
    })))
}

#[derive(Deserialize)]
struct ConfigBody {
    config: crate::config::Config,
}

/// Replaces the live configuration. On a validation failure the previous
/// known-good configuration is kept.
async fn put_config(State(st): State<AppState>, Json(body): Json<ConfigBody>) -> ApiResult {
    crate::config::validate(&body.config)?;
    let old = st.cfg().await;
    *st.config.write().await = body.config.clone();
    st.reload_router().await;
    let restart = old.bind != body.config.bind || old.database_url != body.config.database_url;
    // Persist to the project config so a file edit and a GUI edit agree.
    std::fs::create_dir_all(".friday")?;
    std::fs::write(
        crate::config::PROJECT_CONFIG,
        toml::to_string_pretty(&body.config)?,
    )?;
    st.audit(
        "user",
        "config",
        "update",
        "live configuration replaced",
        None,
    )
    .await;
    crate::scheduler::ensure_default_job(&st).await.ok();
    Ok(Json(json!({
        "saved": true, "written_to": crate::config::PROJECT_CONFIG,
        "restart_required": restart,
        "restart_note": if restart { "bind/database_url changes take effect on restart" } else { "applied live" }
    })))
}

async fn validate_config(Json(body): Json<ConfigBody>) -> ApiResult {
    match crate::config::validate(&body.config) {
        Ok(()) => Ok(Json(json!({"valid": true}))),
        Err(e) => Ok(Json(json!({"valid": false, "error": format!("{e:#}")}))),
    }
}

async fn reset_config(State(st): State<AppState>) -> ApiResult {
    let default = crate::config::Config::default();
    Ok(Json(json!({
        "preview": crate::config::redacted(&default),
        "applied": false,
        "how_to_apply": "PUT /api/config with this object, or delete .friday/config.toml and restart",
        "current": crate::config::redacted(&st.cfg().await)
    })))
}

// ------------------------------------------------------------------ chat

#[derive(Deserialize)]
struct ChatBody {
    #[serde(default)]
    conversation_id: Option<String>,
    message: String,
}

async fn chat(State(st): State<AppState>, Json(b): Json<ChatBody>) -> ApiResult {
    require!(!b.message.trim().is_empty(), "message must not be empty");
    let conv = b.conversation_id.unwrap_or_else(crate::util::uid);
    let outcome = crate::agents::handle_message(&st, &conv, &b.message).await?;
    Ok(Json(json!({
        "conversation_id": conv, "task_id": outcome.task_id, "status": outcome.status,
        "reply": outcome.content, "model": outcome.model, "role": outcome.role,
        "iterations": outcome.iterations, "tool_calls": outcome.tool_calls,
        "tokens": {"prompt": outcome.prompt_tokens, "completion": outcome.completion_tokens}
    })))
}

async fn messages(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult {
    let rows: Vec<(String, String, String)> = sqlx::query_as(
        "SELECT role,content,created_at FROM messages WHERE conversation_id=? ORDER BY created_at",
    )
    .bind(&id)
    .fetch_all(&st.db)
    .await?;
    Ok(Json(
        json!({"conversation_id": id, "messages": rows.into_iter()
        .map(|(role,content,created_at)| json!({"role":role,"content":content,"created_at":created_at}))
        .collect::<Vec<_>>()}),
    ))
}

// ------------------------------------------------------------------ tasks / agents

#[derive(Deserialize)]
struct LimitQuery {
    #[serde(default)]
    limit: Option<i64>,
    #[serde(default)]
    status: Option<String>,
}

async fn tasks(State(st): State<AppState>, Query(q): Query<LimitQuery>) -> ApiResult {
    let ctx = crate::tools::ToolCtx::root(st.clone(), None);
    let v = crate::tools::execute(
        &ctx,
        "list_tasks",
        json!({"limit": q.limit.unwrap_or(50), "status": q.status.unwrap_or_else(|| "all".into())}),
    )
    .await;
    Ok(Json(v.value))
}

async fn task_detail(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult {
    let task: Option<(
        String,
        Option<String>,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        String,
    )> = sqlx::query_as(
        "SELECT id,parent_id,title,status,kind,result,error,created_at FROM tasks WHERE id=?",
    )
    .bind(&id)
    .fetch_optional(&st.db)
    .await?;
    let (tid, parent, title, status, kind, result, error, created_at) =
        task.ok_or_else(|| anyhow::anyhow!("no task {id}"))?;
    let children: Vec<(String, String, String, Option<String>)> = sqlx::query_as(
        "SELECT id,title,status,role FROM tasks WHERE parent_id=? ORDER BY created_at",
    )
    .bind(&id)
    .fetch_all(&st.db)
    .await?;
    let calls: Vec<(String, String, i64, i64, String)> = sqlx::query_as(
        "SELECT tool,permission,ok,duration_ms,created_at FROM tool_calls WHERE task_id=? ORDER BY created_at")
        .bind(&id).fetch_all(&st.db).await?;
    Ok(Json(json!({
        "task": {"id":tid,"parent_id":parent,"title":title,"status":status,"kind":kind,
                 "result":result,"error":error,"created_at":created_at},
        "subtasks": children.into_iter().map(|(id,title,status,role)|
            json!({"id":id,"title":title,"status":status,"role":role})).collect::<Vec<_>>(),
        "tool_calls": calls.into_iter().map(|(tool,permission,ok,ms,at)|
            json!({"tool":tool,"permission":permission,"ok":ok==1,"duration_ms":ms,"created_at":at})).collect::<Vec<_>>(),
        "evidence": crate::evidence::verify(&st, &id, None).await?
    })))
}

async fn cancel(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult {
    let ctx = crate::tools::ToolCtx::root(st.clone(), Some(id.clone()));
    Ok(Json(
        crate::tools::execute(&ctx, "cancel_task", json!({"task_id": id}))
            .await
            .value,
    ))
}

async fn agents(State(st): State<AppState>, Query(q): Query<LimitQuery>) -> ApiResult {
    let ctx = crate::tools::ToolCtx::root(st.clone(), None);
    Ok(Json(
        crate::tools::execute(
            &ctx,
            "get_agent_status",
            json!({"limit": q.limit.unwrap_or(30)}),
        )
        .await
        .value,
    ))
}

// ------------------------------------------------------------------ memory

#[derive(Deserialize)]
struct MemoryQuery {
    #[serde(default)]
    q: Option<String>,
    #[serde(default)]
    include_unapproved: Option<bool>,
}

async fn list_memories(State(st): State<AppState>, Query(q): Query<MemoryQuery>) -> ApiResult {
    if let Some(query) = q.q.filter(|s| !s.is_empty()) {
        return Ok(Json(
            json!({"memories": crate::memory::search(&st, &query, 50).await?}),
        ));
    }
    let sql = if q.include_unapproved.unwrap_or(true) {
        "SELECT id,category,content,source,provenance,confidence,approved,created_at FROM memories ORDER BY created_at DESC LIMIT 200"
    } else {
        "SELECT id,category,content,source,provenance,confidence,approved,created_at FROM memories WHERE approved=1 ORDER BY created_at DESC LIMIT 200"
    };
    let rows: Vec<(String, String, String, String, String, f64, i64, String)> =
        sqlx::query_as(sql).fetch_all(&st.db).await?;
    Ok(Json(json!({"memories": rows.into_iter()
        .map(|(id,category,content,source,provenance,confidence,approved,created_at)|
            json!({"id":id,"category":category,"content":content,"source":source,"provenance":provenance,
                   "confidence":confidence,"approved":approved==1,"created_at":created_at}))
        .collect::<Vec<_>>()})))
}

async fn create_memory(State(st): State<AppState>, Json(b): Json<Value>) -> ApiResult {
    Ok(Json(crate::memory::upsert(&st, &b, true).await?))
}

async fn edit_memory(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<Value>,
) -> ApiResult {
    Ok(Json(
        crate::memory::update(
            &st,
            &id,
            b.get("content")
                .and_then(|c| c.as_str())
                .map(str::to_string),
            b.get("tags").cloned(),
        )
        .await?,
    ))
}

async fn remove_memory(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult {
    Ok(Json(crate::memory::delete(&st, &id).await?))
}

async fn approve_memory(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult {
    Ok(Json(crate::memory::approve(&st, &id).await?))
}

async fn memory_conflicts(State(st): State<AppState>) -> ApiResult {
    Ok(Json(
        json!({"conflicts": crate::memory::conflicts(&st).await?}),
    ))
}

// ------------------------------------------------------------------ research

async fn list_research(State(st): State<AppState>, Query(q): Query<MemoryQuery>) -> ApiResult {
    let query = q.q.unwrap_or_default();
    let items = if query.is_empty() {
        let rows: Vec<(String, String, String, String)> = sqlx::query_as(
            "SELECT id,title,summary,created_at FROM research_documents ORDER BY created_at DESC LIMIT 100")
            .fetch_all(&st.db).await?;
        rows.into_iter()
            .map(|(id, title, summary, created_at)| json!({"id":id,"title":title,"summary":summary,"created_at":created_at}))
            .collect()
    } else {
        crate::research::search(&st, &query, 50).await?
    };
    Ok(Json(
        json!({"research": items, "stale": crate::research::freshness_check(&st).await?}),
    ))
}

async fn get_research(State(st): State<AppState>, Path(id): Path<String>) -> ApiResult {
    Ok(Json(crate::research::get_report(&st, &id).await?))
}

// ------------------------------------------------------------------ skills

async fn list_skills(State(st): State<AppState>) -> ApiResult {
    Ok(Json(json!({
        "skills": crate::skills::list(&st).await?,
        "scopes": {"builtin": "ships with FRIDAY", "user": "available everywhere",
                   "project": "this project only; wins on a name clash"},
        "package_format": {"manifest": "skill.toml", "instructions": "SKILL.md",
                           "optional": ["references/", "scripts/ (never executed by default)"]}
    })))
}

#[derive(Deserialize)]
struct AddSkillBody {
    /// `template` to scaffold a new one, or `path` to import an existing folder.
    #[serde(default)]
    mode: Option<String>,
    #[serde(default)]
    name: Option<String>,
    #[serde(default)]
    description: Option<String>,
    #[serde(default)]
    path: Option<String>,
    #[serde(default)]
    scope: Option<String>,
    #[serde(default)]
    enable: Option<bool>,
}

async fn add_skill(State(st): State<AppState>, Json(b): Json<AddSkillBody>) -> ApiResult {
    let scope = b.scope.unwrap_or_else(|| "project".into());
    match b.mode.as_deref().unwrap_or("template") {
        "template" => {
            let name = b.name.ok_or_else(|| anyhow::anyhow!("name is required"))?;
            Ok(Json(
                crate::skills::create_from_template(
                    &st,
                    &name,
                    b.description.as_deref().unwrap_or("Describe this skill."),
                    &scope,
                )
                .await?,
            ))
        }
        "path" => {
            let p = b
                .path
                .ok_or_else(|| anyhow::anyhow!("path is required for mode=path"))?;
            Ok(Json(
                crate::skills::install(
                    &st,
                    std::path::Path::new(&p),
                    &scope,
                    b.enable.unwrap_or(false),
                )
                .await?,
            ))
        }
        other => Err(ApiError(anyhow::anyhow!(
            "unknown mode {other:?}; use 'template' or 'path'"
        ))),
    }
}

/// Shows what a package declares and what your permissions would allow,
/// before anything is installed or enabled.
async fn inspect_skill(State(st): State<AppState>, Json(b): Json<AddSkillBody>) -> ApiResult {
    let p = b.path.ok_or_else(|| anyhow::anyhow!("path is required"))?;
    Ok(Json(
        crate::skills::inspect(&st, std::path::Path::new(&p)).await?,
    ))
}

async fn rescan_skills(State(st): State<AppState>) -> ApiResult {
    Ok(Json(crate::skills::rescan(&st).await?))
}

#[derive(Deserialize)]
struct EnabledBody {
    enabled: bool,
}

async fn set_skill_enabled(
    State(st): State<AppState>,
    Path(name): Path<String>,
    Json(b): Json<EnabledBody>,
) -> ApiResult {
    Ok(Json(
        crate::skills::set_enabled(&st, &name, b.enabled).await?,
    ))
}

#[derive(Deserialize)]
struct RemoveQuery {
    #[serde(default)]
    delete_files: Option<bool>,
}

async fn remove_skill(
    State(st): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<RemoveQuery>,
) -> ApiResult {
    Ok(Json(
        crate::skills::remove(&st, &name, q.delete_files.unwrap_or(false)).await?,
    ))
}

#[derive(Deserialize)]
struct RunSkillBody {
    #[serde(default)]
    input: Option<String>,
}

async fn run_skill(
    State(st): State<AppState>,
    Path(name): Path<String>,
    Json(b): Json<RunSkillBody>,
) -> ApiResult {
    Ok(Json(
        crate::skills::run(&st, &name, b.input.as_deref(), None).await?,
    ))
}

async fn skill_runs(State(st): State<AppState>, Path(name): Path<String>) -> ApiResult {
    Ok(Json(
        json!({"runs": crate::skills::run_history(&st, Some(&name)).await?}),
    ))
}

// ------------------------------------------------------------------ jobs / briefing

async fn jobs(State(st): State<AppState>) -> ApiResult {
    let ctx = crate::tools::ToolCtx::root(st.clone(), None);
    let mut v = crate::tools::execute(&ctx, "get_job_status", json!({}))
        .await
        .value;
    v["recovery"] = crate::scheduler::recovery_report(&st).await?;
    Ok(Json(v))
}

#[derive(Deserialize)]
struct ForceQuery {
    #[serde(default)]
    force: Option<bool>,
}

async fn run_job(
    State(st): State<AppState>,
    Path(name): Path<String>,
    Query(q): Query<ForceQuery>,
) -> ApiResult {
    Ok(Json(
        crate::scheduler::run_job(&st, &name, q.force.unwrap_or(false)).await?,
    ))
}

#[derive(Deserialize)]
struct DateQuery {
    #[serde(default)]
    date: Option<String>,
}

async fn briefing(State(st): State<AppState>, Query(q): Query<DateQuery>) -> ApiResult {
    match crate::briefing::get(&st, q.date.as_deref()).await? {
        Some(b) => Ok(Json(b)),
        None => Ok(Json(json!({
            "briefing": null,
            "note": "no briefing saved yet; POST /api/jobs/morning-briefing/run to generate one"
        }))),
    }
}

// ------------------------------------------------------------------ approvals / audit

async fn approvals(State(st): State<AppState>) -> ApiResult {
    let rows: Vec<(
        String,
        Option<String>,
        String,
        String,
        String,
        String,
        String,
        String,
    )> = sqlx::query_as(
        "SELECT id,task_id,action,reason,risk,preview,status,created_at FROM approval_requests
         ORDER BY created_at DESC LIMIT 50",
    )
    .fetch_all(&st.db)
    .await?;
    Ok(Json(json!({"approvals": rows.into_iter()
        .map(|(id,task_id,action,reason,risk,preview,status,created_at)|
            json!({"id":id,"task_id":task_id,"action":action,"reason":reason,"risk":risk,
                   "preview":preview,"status":status,"created_at":created_at}))
        .collect::<Vec<_>>()})))
}

#[derive(Deserialize)]
struct DecideBody {
    /// `approved` or `denied`.
    decision: String,
}

async fn decide_approval(
    State(st): State<AppState>,
    Path(id): Path<String>,
    Json(b): Json<DecideBody>,
) -> ApiResult {
    require!(
        matches!(b.decision.as_str(), "approved" | "denied"),
        "decision must be 'approved' or 'denied', got {:?}",
        b.decision
    );
    let n = sqlx::query(
        "UPDATE approval_requests SET status=?, decided_at=? WHERE id=? AND status='pending'",
    )
    .bind(&b.decision)
    .bind(crate::util::now())
    .bind(&id)
    .execute(&st.db)
    .await?
    .rows_affected();
    require!(n == 1, "no pending approval request {id}");
    st.audit("user", "approval", &b.decision, &id, None).await;
    Ok(Json(json!({"id": id, "status": b.decision})))
}

async fn audit(State(st): State<AppState>, Query(q): Query<LimitQuery>) -> ApiResult {
    let ctx = crate::tools::ToolCtx::root(st.clone(), None);
    Ok(Json(
        crate::tools::execute(
            &ctx,
            "get_audit_log",
            json!({"limit": q.limit.unwrap_or(100)}),
        )
        .await
        .value,
    ))
}

// ------------------------------------------------------------------ websocket

async fn events_ws(ws: WebSocketUpgrade, State(st): State<AppState>) -> Response {
    ws.on_upgrade(move |mut socket| async move {
        let mut rx = st.events.subscribe();
        while let Ok(ev) = rx.recv().await {
            let text = match serde_json::to_string(&ev) {
                Ok(t) => t,
                Err(_) => continue,
            };
            if socket
                .send(axum::extract::ws::Message::Text(text))
                .await
                .is_err()
            {
                break;
            }
        }
    })
}
