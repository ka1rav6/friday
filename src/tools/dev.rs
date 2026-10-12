//! N21-N24 plus F9/F10/U8. Commands are executed as argument arrays against an
//! allowlist - never as a shell string.
use super::core::deftool;
use super::*;
use crate::util::{redact, truncate};
use std::process::Stdio;
use tokio::process::Command;

pub fn tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(GetGitStatus),
        Arc::new(GetGitDiff),
        Arc::new(RunProjectTests),
        Arc::new(RunBuild),
        Arc::new(SummarizeGitChanges),
        Arc::new(DetectProjectDrift),
    ]
}

/// Runs an allowlisted executable with a fixed argument array inside `cwd`.
async fn run_allowlisted(
    ctx: &ToolCtx,
    cwd: &std::path::Path,
    program: &str,
    args: &[&str],
    timeout_secs: u64,
) -> Result<Value> {
    let cfg = ctx.state.cfg().await;
    anyhow::ensure!(
        cfg.permissions
            .command_allowlist
            .iter()
            .any(|c| c == program),
        "executable {program:?} is not in permissions.command_allowlist ({:?})",
        cfg.permissions.command_allowlist
    );
    let started = std::time::Instant::now();
    let child = Command::new(program)
        .args(args)
        .current_dir(cwd)
        .env("CI", "1")
        .env_remove("NEBIUS_API_KEY")
        .env_remove("TAVILY_API_KEY")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .map_err(|e| anyhow::anyhow!("failed to start {program}: {e}"))?;
    let out = tokio::time::timeout(
        std::time::Duration::from_secs(timeout_secs),
        child.wait_with_output(),
    )
    .await
    .map_err(|_| anyhow::anyhow!("{program} {args:?} timed out after {timeout_secs}s"))??;
    let cap = cfg.permissions.max_tool_output_bytes / 2;
    Ok(json!({
        "program": program, "args": args,
        "exit_code": out.status.code(),
        "success": out.status.success(),
        "duration_ms": started.elapsed().as_millis(),
        "stdout": truncate(&redact(&String::from_utf8_lossy(&out.stdout)), cap),
        "stderr": truncate(&redact(&String::from_utf8_lossy(&out.stderr)), cap)
    }))
}

deftool!(
    GetGitStatus,
    "get_git_status",
    "Branch, staged and modified files in an approved repository.",
    Permission::ReadOnly,
    20_000,
    false,
    obj(json!({"workspace":{"type":"string"}}), &["workspace"]),
    json!({"branch":"string","files":[]}),
    git_status
);
async fn git_status(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let ws = workspace(&ctx.state, &s(&a, "workspace")?).await?;
    let branch = run_allowlisted(
        ctx,
        &ws.path,
        "git",
        &["rev-parse", "--abbrev-ref", "HEAD"],
        20,
    )
    .await?;
    let status = run_allowlisted(ctx, &ws.path, "git", &["status", "--porcelain=v1"], 20).await?;
    let files: Vec<Value> = status["stdout"]
        .as_str()
        .unwrap_or("")
        .lines()
        .filter(|l| l.len() > 3)
        .map(|l| json!({"state": l[..2].trim(), "path": l[3..].trim()}))
        .collect();
    Ok(json!({
        "workspace": ws.id,
        "branch": branch["stdout"].as_str().unwrap_or("").trim(),
        "changed_files": files.len(), "files": files
    }))
}

deftool!(
    GetGitDiff,
    "get_git_diff",
    "Diff of the working tree (or one path) in an approved repository.",
    Permission::ReadOnly,
    30_000,
    false,
    obj(
        json!({"workspace":{"type":"string"},"path":{"type":"string"},"staged":{"type":"boolean","default":false}}),
        &["workspace"]
    ),
    json!({"diff":"string"}),
    git_diff
);
async fn git_diff(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let ws = workspace(&ctx.state, &s(&a, "workspace")?).await?;
    let mut args: Vec<&str> = vec!["diff", "--stat", "--patch"];
    if b(&a, "staged", false) {
        args.push("--cached");
    }
    let path = s_opt(&a, "path");
    if let Some(p) = &path {
        // Validate the path stays inside the workspace before handing it to git.
        crate::util::safe_join(&ws.path, p)?;
        args.push("--");
        args.push(p);
    }
    let out = run_allowlisted(ctx, &ws.path, "git", &args, 30).await?;
    Ok(json!({"workspace": ws.id, "diff": out["stdout"], "stderr": out["stderr"]}))
}

deftool!(
    RunProjectTests,
    "run_project_tests",
    "Run the approved test command for a workspace and return the real exit code and output.",
    Permission::LocalMutation,
    600_000,
    false,
    obj(
        json!({"workspace":{"type":"string"},
        "profile":{"type":"string","enum":["cargo","npm"],"default":"cargo"},
        "filter":{"type":"string"}}),
        &["workspace"]
    ),
    json!({"success":"bool","exit_code":"int","stdout":"string"}),
    run_tests
);
async fn run_tests(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let ws = workspace(&ctx.state, &s(&a, "workspace")?).await?;
    let profile = s_opt(&a, "profile").unwrap_or_else(|| "cargo".into());
    let filter = s_opt(&a, "filter").unwrap_or_default();
    anyhow::ensure!(
        filter
            .chars()
            .all(|c| c.is_alphanumeric() || "_-:.".contains(c)),
        "filter may only contain alphanumerics, '_', '-', ':' and '.'"
    );
    let out = match profile.as_str() {
        "cargo" => {
            let mut args = vec!["test", "--quiet"];
            if !filter.is_empty() {
                args.push(&filter);
            }
            run_allowlisted(ctx, &ws.path, "cargo", &args, 600).await?
        }
        "npm" => run_allowlisted(ctx, &ws.path, "npm", &["test", "--silent"], 600).await?,
        other => bail!("unknown test profile {other:?}"),
    };
    if let Some(t) = &ctx.task_id {
        ctx.state
            .record_evidence(
                t,
                "tests executed",
                "test_run",
                None,
                &format!("profile={profile} exit={:?}", out["exit_code"]),
                out["success"] == json!(true),
            )
            .await?;
    }
    Ok(out)
}

deftool!(
    RunBuild,
    "run_build",
    "Build an approved workspace and return real diagnostics.",
    Permission::LocalMutation,
    600_000,
    false,
    obj(
        json!({"workspace":{"type":"string"},"profile":{"type":"string","enum":["cargo","npm"],"default":"cargo"}}),
        &["workspace"]
    ),
    json!({"success":"bool","stderr":"string"}),
    run_build
);
async fn run_build(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let ws = workspace(&ctx.state, &s(&a, "workspace")?).await?;
    let out = match s_opt(&a, "profile")
        .unwrap_or_else(|| "cargo".into())
        .as_str()
    {
        "cargo" => run_allowlisted(ctx, &ws.path, "cargo", &["build", "--quiet"], 600).await?,
        "npm" => run_allowlisted(ctx, &ws.path, "npm", &["run", "build", "--silent"], 600).await?,
        other => bail!("unknown build profile {other:?}"),
    };
    if let Some(t) = &ctx.task_id {
        ctx.state
            .record_evidence(
                t,
                "build executed",
                "build",
                None,
                &format!("exit={:?}", out["exit_code"]),
                out["success"] == json!(true),
            )
            .await?;
    }
    Ok(out)
}

deftool!(
    SummarizeGitChanges,
    "summarize_git_changes",
    "Recent commits plus working-tree state for an approved repository.",
    Permission::ReadOnly,
    30_000,
    false,
    obj(
        json!({"workspace":{"type":"string"},"count":{"type":"integer","default":10}}),
        &["workspace"]
    ),
    json!({"commits":[],"working_tree":{}}),
    summarize_git
);
async fn summarize_git(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let ws = workspace(&ctx.state, &s(&a, "workspace")?).await?;
    let count = n(&a, "count", 10).clamp(1, 100).to_string();
    let log = run_allowlisted(
        ctx,
        &ws.path,
        "git",
        &["log", "--oneline", "-n", &count],
        20,
    )
    .await?;
    let status = git_status(ctx, json!({"workspace": ws.id})).await?;
    Ok(json!({
        "commits": log["stdout"].as_str().unwrap_or("").lines().collect::<Vec<_>>(),
        "working_tree": status
    }))
}

deftool!(
    DetectProjectDrift, "detect_project_drift",
    "Compare a repository's current state against saved project memories and goals, and report gaps.",
    Permission::ReadOnly, 30_000, false,
    obj(json!({"workspace":{"type":"string"}}), &["workspace"]),
    json!({"goals":[],"current":{}}), drift
);
async fn drift(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let ws_id = s(&a, "workspace")?;
    let goals: Vec<(String, String)> = sqlx::query_as(
        "SELECT id, content FROM memories WHERE approved=1 AND category='project' ORDER BY created_at")
        .fetch_all(&ctx.state.db).await?;
    let status = git_status(ctx, json!({"workspace": ws_id})).await?;
    Ok(json!({
        "saved_goals": goals.into_iter().map(|(id,content)| json!({"id":id,"content":content})).collect::<Vec<_>>(),
        "repository": status,
        "instruction": "List requirements with no visible implementation and changes with no matching saved goal. \
                        Base every statement on the data above."
    }))
}
