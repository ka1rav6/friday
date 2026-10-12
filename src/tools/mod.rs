//! Tool registry. The model may *request* a tool; the runtime decides whether
//! it runs. Every invocation is permission-checked, timed, size-bounded and
//! written to `tool_calls` + `audit_log`.
use crate::state::{AppState, Event};
use anyhow::{bail, Result};
use async_trait::async_trait;
use serde::Serialize;
use serde_json::{json, Value};
use std::collections::{HashMap, HashSet};
use std::sync::Arc;
use std::time::Duration;

pub mod agentic;
pub mod core;
pub mod dev;
pub mod personal;
pub mod web;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
// `ExternalSideEffect` and `Privileged` are reserved for connectors that send
// mail or run privileged commands; both default to denied in PermissionConfig.
#[allow(dead_code)]
pub enum Permission {
    ReadOnly,
    LocalMutation,
    Network,
    ExternalSideEffect,
    Privileged,
}

impl Permission {
    pub fn as_str(&self) -> &'static str {
        match self {
            Permission::ReadOnly => "read_only",
            Permission::LocalMutation => "local_mutation",
            Permission::Network => "network",
            Permission::ExternalSideEffect => "external_side_effect",
            Permission::Privileged => "privileged",
        }
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct ToolDefinition {
    pub name: &'static str,
    pub description: &'static str,
    pub permission: Permission,
    pub timeout_ms: u64,
    /// JSON Schema for arguments.
    pub schema: Value,
    /// Shape of a successful result, for documentation and UI.
    pub output: Value,
    /// Tool needs the network, so it is unavailable in offline mode.
    pub network: bool,
}

/// Context handed to a tool. `allowed` is `Some` for subagents, which receive
/// only the tools their parent granted; a child can never widen this set.
pub struct ToolCtx {
    pub state: AppState,
    pub task_id: Option<String>,
    pub agent_run_id: Option<String>,
    pub allowed: Option<HashSet<String>>,
    pub depth: u8,
}

impl ToolCtx {
    pub fn root(state: AppState, task_id: Option<String>) -> Self {
        Self {
            state,
            task_id,
            agent_run_id: None,
            allowed: None,
            depth: 0,
        }
    }
}

#[async_trait]
pub trait Tool: Send + Sync {
    fn def(&self) -> ToolDefinition;
    async fn run(&self, ctx: &ToolCtx, args: Value) -> Result<Value>;
}

pub struct ToolRegistry {
    tools: HashMap<&'static str, Arc<dyn Tool>>,
}

impl ToolRegistry {
    pub fn get(&self, name: &str) -> Option<Arc<dyn Tool>> {
        self.tools.get(name).cloned()
    }
    #[allow(dead_code)] // used by the registry integrity tests
    pub fn names(&self) -> Vec<&'static str> {
        let mut v: Vec<_> = self.tools.keys().copied().collect();
        v.sort_unstable();
        v
    }
    pub fn defs(&self) -> Vec<ToolDefinition> {
        let mut v: Vec<_> = self.tools.values().map(|t| t.def()).collect();
        v.sort_by_key(|d| d.name);
        v
    }

    /// OpenAI-format tool schemas, filtered to what is actually usable right
    /// now (offline mode and the subagent's grant).
    pub fn schemas_for(&self, allowed: Option<&HashSet<String>>, offline: bool) -> Vec<Value> {
        self.defs()
            .into_iter()
            .filter(|d| allowed.map_or(true, |a| a.contains(d.name)))
            .filter(|d| !(offline && d.network))
            .map(|d| {
                json!({"type":"function","function":{
                    "name": d.name, "description": d.description, "parameters": d.schema}})
            })
            .collect()
    }
}

fn insert(m: &mut HashMap<&'static str, Arc<dyn Tool>>, t: Arc<dyn Tool>) {
    let name = t.def().name;
    debug_assert!(!m.contains_key(name), "duplicate tool {name}");
    m.insert(name, t);
}

pub fn registry() -> ToolRegistry {
    let mut m: HashMap<&'static str, Arc<dyn Tool>> = HashMap::new();
    for t in core::tools() {
        insert(&mut m, t);
    }
    for t in web::tools() {
        insert(&mut m, t);
    }
    for t in dev::tools() {
        insert(&mut m, t);
    }
    for t in agentic::tools() {
        insert(&mut m, t);
    }
    for t in personal::tools() {
        insert(&mut m, t);
    }
    ToolRegistry { tools: m }
}

#[derive(Debug, Serialize)]
pub struct ToolOutcome {
    pub call_id: String,
    pub ok: bool,
    pub value: Value,
    pub duration_ms: u64,
}

/// The single entry point for tool execution. Nothing else may call
/// `Tool::run` directly.
pub async fn execute(ctx: &ToolCtx, name: &str, args: Value) -> ToolOutcome {
    let call_id = crate::util::uid();
    let started = std::time::Instant::now();
    let result = execute_inner(ctx, name, &args).await;
    let duration_ms = started.elapsed().as_millis() as u64;

    let (ok, value) = match &result {
        Ok(v) => (true, v.clone()),
        Err(e) => (
            false,
            json!({"error": crate::util::redact(&format!("{e:#}"))}),
        ),
    };
    let permission = ctx
        .state
        .registry
        .get(name)
        .map(|t| t.def().permission.as_str().to_string())
        .unwrap_or_else(|| "unknown".into());

    let _ = sqlx::query(
        "INSERT INTO tool_calls (id, task_id, agent_run_id, tool, args, result, ok, permission, duration_ms, error, created_at)
         VALUES (?,?,?,?,?,?,?,?,?,?,?)",
    )
    .bind(&call_id)
    .bind(ctx.task_id.as_deref())
    .bind(ctx.agent_run_id.as_deref())
    .bind(name)
    .bind(crate::util::truncate(&crate::util::redact(&args.to_string()), 8000))
    .bind(crate::util::truncate(&crate::util::redact(&value.to_string()), 16000))
    .bind(ok as i64)
    .bind(&permission)
    .bind(duration_ms as i64)
    .bind(if ok { None } else { Some(value["error"].as_str().unwrap_or("error").to_string()) })
    .bind(crate::util::now())
    .execute(&ctx.state.db)
    .await;

    ctx.state
        .audit(
            ctx.agent_run_id.as_deref().unwrap_or("user"),
            "tool",
            name,
            &format!("permission={permission} ok={ok} {}ms", duration_ms),
            ctx.task_id.as_deref(),
        )
        .await;

    ctx.state.emit(Event::ToolCall {
        task_id: ctx.task_id.clone(),
        tool: name.to_string(),
        ok,
        summary: crate::util::truncate(&value.to_string(), 300),
    });

    ToolOutcome {
        call_id,
        ok,
        value,
        duration_ms,
    }
}

async fn execute_inner(ctx: &ToolCtx, name: &str, args: &Value) -> Result<Value> {
    let tool =
        ctx.state.registry.get(name).ok_or_else(|| {
            anyhow::anyhow!("unknown tool {name}; call only tools offered to you")
        })?;
    let def = tool.def();

    // 1. Subagent grant. A child cannot widen its own permissions.
    if let Some(allowed) = &ctx.allowed {
        if !allowed.contains(name) {
            bail!("tool {name} is not in this agent's granted tool set");
        }
    }

    // 2. Policy from configuration.
    let cfg = ctx.state.cfg().await;
    let p = &cfg.permissions;
    let denied = match def.permission {
        Permission::ReadOnly => false,
        Permission::LocalMutation => !p.allow_local_mutation,
        Permission::Network => !p.allow_network,
        Permission::ExternalSideEffect => !p.allow_external_side_effect,
        Permission::Privileged => !p.allow_privileged,
    };
    if denied {
        bail!(
            "permission {} is disabled in configuration; enable permissions.allow_{} to use {name}",
            def.permission.as_str(),
            match def.permission {
                Permission::LocalMutation => "local_mutation",
                Permission::Network => "network",
                Permission::ExternalSideEffect => "external_side_effect",
                _ => "privileged",
            }
        );
    }

    // 3. Offline routing. Fail honestly rather than silently going online.
    if def.network && cfg.mode == "offline" {
        bail!(
            "{name} requires network access but FRIDAY is in offline mode. \
               Switch mode to auto/online, or use a local tool such as search_knowledge."
        );
    }

    // 4. Required argument check against the declared schema.
    validate_args(&def, args)?;

    // 5. Bounded execution.
    let timeout = Duration::from_millis(def.timeout_ms.min(cfg.agents.tool_timeout_secs * 1000));
    let fut = tool.run(ctx, args.clone());
    let value = tokio::time::timeout(timeout, fut)
        .await
        .map_err(|_| anyhow::anyhow!("tool {name} timed out after {:?}", timeout))??;

    // 6. Output size bound.
    let s = value.to_string();
    if s.len() > p.max_tool_output_bytes {
        return Ok(json!({
            "truncated": true,
            "note": format!("result exceeded max_tool_output_bytes ({}); showing a prefix", p.max_tool_output_bytes),
            "preview": crate::util::truncate(&s, p.max_tool_output_bytes)
        }));
    }
    Ok(value)
}

pub fn validate_args(def: &ToolDefinition, args: &Value) -> Result<()> {
    let Some(required) = def.schema.get("required").and_then(|r| r.as_array()) else {
        return Ok(());
    };
    let obj = args.as_object();
    for r in required {
        let key = r.as_str().unwrap_or_default();
        let present = obj
            .map(|o| o.get(key).is_some_and(|v| !v.is_null()))
            .unwrap_or(false);
        if !present {
            bail!("tool {}: missing required argument `{key}`", def.name);
        }
    }
    Ok(())
}

// ------------------------------------------------------------------ helpers

pub fn s(args: &Value, key: &str) -> Result<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .ok_or_else(|| anyhow::anyhow!("argument `{key}` must be a string"))
}
pub fn s_opt(args: &Value, key: &str) -> Option<String> {
    args.get(key)
        .and_then(|v| v.as_str())
        .map(str::to_string)
        .filter(|x| !x.is_empty())
}
pub fn n(args: &Value, key: &str, default: i64) -> i64 {
    args.get(key).and_then(|v| v.as_i64()).unwrap_or(default)
}
pub fn b(args: &Value, key: &str, default: bool) -> bool {
    args.get(key).and_then(|v| v.as_bool()).unwrap_or(default)
}
pub fn obj(props: Value, required: &[&str]) -> Value {
    json!({"type":"object","properties":props,"required":required})
}
pub fn str_list(args: &Value, key: &str) -> Vec<String> {
    args.get(key)
        .and_then(|v| v.as_array())
        .map(|a| {
            a.iter()
                .filter_map(|x| x.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default()
}

/// Resolves a configured workspace by id.
pub async fn workspace(state: &AppState, id: &str) -> Result<crate::config::Workspace> {
    let cfg = state.cfg().await;
    cfg.workspaces
        .iter()
        .find(|w| w.id == id)
        .cloned()
        .ok_or_else(|| {
            anyhow::anyhow!(
                "unknown workspace {id:?}. Configured workspaces: [{}]. Add one under [[workspaces]] in .friday/config.toml.",
                cfg.workspaces.iter().map(|w| w.id.as_str()).collect::<Vec<_>>().join(", ")
            )
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn registry_has_no_duplicates_and_core_tools_present() {
        let r = registry();
        let names = r.names();
        for want in [
            "get_system_status",
            "search_local_files",
            "read_local_file",
            "search_knowledge",
            "propose_memory",
            "save_memory",
            "delete_memory",
            "get_current_time",
            "create_task",
            "search_web",
            "fetch_web_page",
            "request_approval",
            "spawn_subagent",
            "cancel_task",
            "get_audit_log",
            "verify_task_completion",
            "offline_capability_report",
        ] {
            assert!(names.contains(&want), "missing tool {want}");
        }
        assert!(
            names.len() >= 35,
            "expected a full catalogue, got {}",
            names.len()
        );
    }

    #[test]
    fn missing_required_argument_is_rejected() {
        let r = registry();
        let def = r.get("read_local_file").unwrap().def();
        assert!(validate_args(&def, &json!({})).is_err());
        assert!(validate_args(&def, &json!({"workspace":"w","path":"a"})).is_ok());
    }

    #[test]
    fn offline_filters_network_tools() {
        let r = registry();
        let on = r.schemas_for(None, false).len();
        let off = r.schemas_for(None, true).len();
        assert!(off < on);
        let names: Vec<String> = r
            .schemas_for(None, true)
            .iter()
            .map(|v| v["function"]["name"].as_str().unwrap().to_string())
            .collect();
        assert!(!names.contains(&"search_web".to_string()));
        assert!(names.contains(&"search_knowledge".to_string()));
    }

    #[test]
    fn grant_set_restricts_tools() {
        let r = registry();
        let allowed: HashSet<String> = ["search_knowledge".to_string()].into_iter().collect();
        assert_eq!(r.schemas_for(Some(&allowed), false).len(), 1);
    }
}
