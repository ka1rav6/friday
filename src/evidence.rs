//! Proof-Carrying Actions. A task may only be reported complete when the
//! recorded tool evidence supports it. The check is deterministic: it reads
//! `tool_calls` and `evidence`, never asks a model whether the work looks done.
use crate::state::AppState;
use anyhow::Result;
use serde_json::{json, Value};

pub async fn verify(state: &AppState, task_id: &str, claim: Option<&str>) -> Result<Value> {
    let calls: Vec<(String, String, i64, Option<String>, String)> = sqlx::query_as(
        "SELECT id, tool, ok, error, created_at FROM tool_calls WHERE task_id=? ORDER BY created_at")
        .bind(task_id).fetch_all(&state.db).await?;
    let ev: Vec<(String, String, String, i64)> = sqlx::query_as(
        "SELECT claim, kind, detail, verified FROM evidence WHERE task_id=? ORDER BY created_at",
    )
    .bind(task_id)
    .fetch_all(&state.db)
    .await?;

    let succeeded: Vec<&str> = calls
        .iter()
        .filter(|c| c.2 == 1)
        .map(|c| c.1.as_str())
        .collect();
    let failed: Vec<Value> = calls
        .iter()
        .filter(|c| c.2 == 0)
        .map(|c| json!({"tool": c.1, "error": c.3}))
        .collect();

    let mut missing = Vec::new();
    if succeeded.is_empty() {
        missing
            .push("no tool call succeeded for this task, so nothing has been verified".to_string());
    }
    if !failed.is_empty() {
        missing.push(format!(
            "{} tool call(s) failed and must be reported, not hidden",
            failed.len()
        ));
    }
    // A claim that asserts a mutation needs a mutating tool in the record.
    if let Some(c) = claim {
        let lower = c.to_lowercase();
        const MUTATION_WORDS: &[&str] = &[
            "created",
            "wrote",
            "saved",
            "updated",
            "deleted",
            "built",
            "ran",
            "sent",
            "scheduled",
        ];
        if MUTATION_WORDS.iter().any(|w| lower.contains(w)) && succeeded.is_empty() {
            missing.push(
                "the claim asserts an action was performed, but no successful tool call exists"
                    .into(),
            );
        }
    }

    let verified = missing.is_empty();
    Ok(json!({
        "task_id": task_id,
        "claim": claim,
        "verified": verified,
        "successful_tools": succeeded,
        "failed_tools": failed,
        "evidence": ev.into_iter().map(|(claim,kind,detail,v)|
            json!({"claim":claim,"kind":kind,"detail":detail,"verified":v==1})).collect::<Vec<_>>(),
        "missing": missing,
        "instruction": if verified {
            "Evidence supports the claim. You may report completion, citing the tools used."
        } else {
            "Do NOT report this task as complete. Tell the user exactly what is unverified."
        }
    }))
}

/// Short human-readable evidence footer appended to an agent's final answer.
pub async fn footer(state: &AppState, task_id: &str) -> Result<String> {
    let calls: Vec<(String, i64, i64)> = sqlx::query_as(
        "SELECT tool, ok, duration_ms FROM tool_calls WHERE task_id=? ORDER BY created_at",
    )
    .bind(task_id)
    .fetch_all(&state.db)
    .await?;
    if calls.is_empty() {
        return Ok(String::new());
    }
    let parts: Vec<String> = calls
        .iter()
        .map(|(tool, ok, ms)| format!("{tool}{} {ms}ms", if *ok == 1 { "" } else { " (failed)" }))
        .collect();
    let failed = calls.iter().filter(|c| c.1 == 0).count();
    Ok(format!(
        "\n\n---\n**Evidence** ({} tool call{}{}): {}",
        calls.len(),
        if calls.len() == 1 { "" } else { "s" },
        if failed > 0 {
            format!(", {failed} failed")
        } else {
            String::new()
        },
        parts.join(" · ")
    ))
}
