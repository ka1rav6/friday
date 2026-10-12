//! Multi-agent orchestration, scheduling, skills, and the safety-preview tools
//! (N14-N16, N28-N31, F21-F22, U9-U17, U22, U23, U26).
use super::core::deftool;
use super::*;
use crate::agents::{self, SubagentSpec};
use crate::util::{now, truncate, uid};

pub fn tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(SpawnSubagent),
        Arc::new(GetSubagentResult),
        Arc::new(GetAgentStatus),
        Arc::new(CancelTask),
        Arc::new(ScheduleJob),
        Arc::new(RunJobNow),
        Arc::new(GetJobStatus),
        Arc::new(RunSkill),
        Arc::new(CreateSkillDraft),
        Arc::new(EstimateWorkflowCost),
        Arc::new(DetectDuplicateWork),
        Arc::new(CreateDecisionRecord),
        Arc::new(AuditMemoryConflicts),
        Arc::new(SafeActionPreview),
        Arc::new(PrivacyImpactPreview),
        Arc::new(RunAdversarialReview),
        Arc::new(ScheduledWorkflowRecovery),
        Arc::new(PersonalAutomationReadiness),
    ]
}

// ------------------------------------------------------------ N29 / N30 / N28 / N31

deftool!(
    SpawnSubagent, "spawn_subagent",
    "Launch 1-5 bounded specialist subagents on INDEPENDENT subtasks and wait for all of them. \
     Only use this when the subtasks genuinely do not depend on each other; otherwise do the work yourself. \
     Subagents cannot delegate further.",
    Permission::ReadOnly, 600_000, false,
    obj(json!({"subagents":{"type":"array","minItems":1,"maxItems":5,"items":{"type":"object","properties":{
            "role":{"type":"string","enum":["planner","researcher","summarizer","coder","verifier","memory","briefing","offline"]},
            "objective":{"type":"string","description":"a self-contained subtask"},
            "allowed_tools":{"type":"array","items":{"type":"string"},"description":"narrows the role default; cannot widen it"},
            "output_schema":{"type":"object"},
            "time_budget_secs":{"type":"integer"}},
        "required":["role","objective"]}}}), &["subagents"]),
    json!({"estimate":{},"results":[{"role":"string","status":"string","result":"string"}]}),
    spawn_subagent
);
async fn spawn_subagent(ctx: &ToolCtx, a: Value) -> Result<Value> {
    anyhow::ensure!(
        ctx.depth == 0,
        "nested delegation is disabled: a subagent may not spawn further subagents"
    );
    let task_id = ctx
        .task_id
        .clone()
        .ok_or_else(|| anyhow::anyhow!("spawn_subagent requires a parent task"))?;
    let specs: Vec<SubagentSpec> = serde_json::from_value(a["subagents"].clone())
        .map_err(|e| anyhow::anyhow!("invalid subagents array: {e}"))?;
    let cfg = ctx.state.cfg().await;
    anyhow::ensure!(
        specs.len() <= cfg.agents.hard_max_subagents,
        "requested {} subagents; the configured hard maximum is {}",
        specs.len(),
        cfg.agents.hard_max_subagents
    );
    let mut estimate = agents::estimate_cost(&cfg, specs.len());
    estimate["models"] = Value::Array(
        specs
            .iter()
            .map(|sp| json!({"role": sp.role, "model": agents::model_for_role(&cfg, &sp.role)}))
            .collect(),
    );
    let cancel = ctx
        .state
        .cancels
        .read()
        .await
        .get(&task_id)
        .cloned()
        .unwrap_or_default();

    let started = std::time::Instant::now();
    let results = agents::fan_out(&ctx.state, &task_id, specs, ctx.allowed.clone(), cancel).await?;
    let ok = results
        .iter()
        .filter(|r| r["status"] == json!("completed"))
        .count();

    ctx.state
        .record_evidence(
            &task_id,
            &format!("{ok} of {} subagents completed", results.len()),
            "subagent_fanout",
            None,
            &results
                .iter()
                .map(|r| format!("{}:{}", r["role"], r["status"]))
                .collect::<Vec<_>>()
                .join(", "),
            ok > 0,
        )
        .await?;

    Ok(json!({
        "estimate": estimate,
        "actual": {"subagents": results.len(), "completed": ok,
                   "wall_clock_secs": started.elapsed().as_secs()},
        "results": results,
        "instruction": "Merge these results yourself. Report any subagent that failed; do not silently drop it."
    }))
}

deftool!(
    GetSubagentResult,
    "get_subagent_result",
    "Retrieve a finished subagent's stored result by task or run id.",
    Permission::ReadOnly,
    5_000,
    false,
    obj(
        json!({"task_id":{"type":"string"},"run_id":{"type":"string"}}),
        &[]
    ),
    json!({"status":"string","result":"string"}),
    get_subagent_result
);
async fn get_subagent_result(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let (col, val) = match (s_opt(&a, "run_id"), s_opt(&a, "task_id")) {
        (Some(r), _) => ("id", r),
        (None, Some(t)) => ("task_id", t),
        _ => bail!("provide either task_id or run_id"),
    };
    let q = format!("SELECT id,task_id,role,status,result,error,iterations,tool_calls FROM agent_runs WHERE {col}=?");
    let row: Option<(
        String,
        String,
        String,
        String,
        Option<String>,
        Option<String>,
        i64,
        i64,
    )> = sqlx::query_as(&q)
        .bind(&val)
        .fetch_optional(&ctx.state.db)
        .await?;
    let (id, task_id, role, status, result, error, it, tc) =
        row.ok_or_else(|| anyhow::anyhow!("no agent run for {col}={val}"))?;
    Ok(
        json!({"run_id":id,"task_id":task_id,"role":role,"status":status,
              "result":result,"error":error,"iterations":it,"tool_calls":tc}),
    )
}

deftool!(
    GetAgentStatus,
    "get_agent_status",
    "Active and recent agent runs with their roles, status and elapsed time.",
    Permission::ReadOnly,
    5_000,
    false,
    obj(
        json!({"task_id":{"type":"string"},"limit":{"type":"integer","default":20}}),
        &[]
    ),
    json!({"agents":[]}),
    get_agent_status
);
async fn get_agent_status(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let limit = n(&a, "limit", 20).clamp(1, 100);
    let rows: Vec<(String, String, Option<String>, String, String, String, Option<String>, i64, i64)> =
        match s_opt(&a, "task_id") {
            Some(t) => sqlx::query_as(
                "SELECT id,task_id,parent_task_id,role,status,started_at,finished_at,iterations,tool_calls
                 FROM agent_runs WHERE task_id=? OR parent_task_id=? ORDER BY started_at DESC LIMIT ?")
                .bind(&t).bind(&t).bind(limit).fetch_all(&ctx.state.db).await?,
            None => sqlx::query_as(
                "SELECT id,task_id,parent_task_id,role,status,started_at,finished_at,iterations,tool_calls
                 FROM agent_runs ORDER BY started_at DESC LIMIT ?")
                .bind(limit).fetch_all(&ctx.state.db).await?,
        };
    let cfg = ctx.state.cfg().await;
    Ok(json!({
        "limits": {"max_concurrent_subagents": cfg.agents.max_concurrent_subagents,
                   "hard_max": cfg.agents.hard_max_subagents, "max_depth": cfg.agents.max_subagent_depth},
        "available_permits": ctx.state.subagent_sem.available_permits(),
        "agents": rows.into_iter().map(|(id,task,parent,role,status,started,finished,it,tc)|
            json!({"run_id":id,"task_id":task,"parent_task_id":parent,"role":role,"status":status,
                   "started_at":started,"finished_at":finished,"iterations":it,"tool_calls":tc})).collect::<Vec<_>>()
    }))
}

deftool!(
    CancelTask,
    "cancel_task",
    "Cancel a task; cancellation propagates to its child subagents.",
    Permission::LocalMutation,
    5_000,
    false,
    obj(json!({"task_id":{"type":"string"}}), &["task_id"]),
    json!({"cancelled":"bool"}),
    cancel_task
);
async fn cancel_task(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let id = s(&a, "task_id")?;
    let signalled = ctx.state.cancel(&id).await;
    sqlx::query(
        "UPDATE tasks SET status='cancelled', updated_at=?, finished_at=?
         WHERE (id=? OR parent_id=?) AND status IN ('pending','running')",
    )
    .bind(now())
    .bind(now())
    .bind(&id)
    .bind(&id)
    .execute(&ctx.state.db)
    .await?;
    ctx.state
        .audit("user", "task", "cancel", &id, Some(&id))
        .await;
    Ok(
        json!({"task_id": id, "cancelled": true, "signal_delivered": signalled,
              "note": if signalled { "running agents were signalled to stop" } else { "no live agent held this task; state marked cancelled" }}),
    )
}

// ------------------------------------------------------------ N14-N16 scheduling

deftool!(
    ScheduleJob, "schedule_job",
    "Schedule a registered skill to run daily at a given local time. Only registered, enabled skills may run automatically.",
    Permission::LocalMutation, 5_000, false,
    obj(json!({"name":{"type":"string"},"skill":{"type":"string"},
        "at_time":{"type":"string","description":"HH:MM in the job timezone"},
        "timezone":{"type":"string"},"enabled":{"type":"boolean","default":true}}), &["name","skill","at_time"]),
    json!({"id":"string","next_run_at":"string"}), schedule_job
);
async fn schedule_job(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let skill = s(&a, "skill")?;
    let known = crate::skills::is_runnable(&ctx.state, &skill).await?;
    anyhow::ensure!(
        known,
        "skill {skill:?} is not registered and enabled; nothing else may run on a schedule"
    );
    let at = s(&a, "at_time")?;
    anyhow::ensure!(
        crate::config::parse_hhmm(&at).is_some(),
        "at_time must be HH:MM, got {at:?}"
    );
    let cfg = ctx.state.cfg().await;
    let tz = s_opt(&a, "timezone").unwrap_or(cfg.timezone);
    tz.parse::<chrono_tz::Tz>()
        .map_err(|_| anyhow::anyhow!("unknown timezone {tz:?}"))?;
    let id = uid();
    let next = crate::scheduler::next_run(&at, &tz)?;
    sqlx::query(
        "INSERT INTO scheduled_jobs (id,name,skill,at_time,timezone,enabled,next_run_at,created_at)
         VALUES (?,?,?,?,?,?,?,?)
         ON CONFLICT(name) DO UPDATE SET skill=excluded.skill, at_time=excluded.at_time,
            timezone=excluded.timezone, enabled=excluded.enabled, next_run_at=excluded.next_run_at",
    )
    .bind(&id)
    .bind(s(&a, "name")?)
    .bind(&skill)
    .bind(&at)
    .bind(&tz)
    .bind(b(&a, "enabled", true) as i64)
    .bind(&next)
    .bind(now())
    .execute(&ctx.state.db)
    .await?;
    Ok(
        json!({"id": id, "skill": skill, "at_time": at, "timezone": tz, "next_run_at": next,
              "caveat": "This is a local scheduler: it only fires while the FRIDAY service is running."}),
    )
}

deftool!(
    RunJobNow,
    "run_job_now",
    "Run a scheduled job immediately. Idempotent per job per day unless forced.",
    Permission::LocalMutation,
    600_000,
    false,
    obj(
        json!({"job":{"type":"string","description":"job name or id"},"force":{"type":"boolean","default":false}}),
        &["job"]
    ),
    json!({"run_id":"string","status":"string"}),
    run_job_now
);
async fn run_job_now(ctx: &ToolCtx, a: Value) -> Result<Value> {
    crate::scheduler::run_job(&ctx.state, &s(&a, "job")?, b(&a, "force", false)).await
}

deftool!(
    GetJobStatus,
    "get_job_status",
    "Scheduled job definitions and their recent run history, including failures.",
    Permission::ReadOnly,
    5_000,
    false,
    obj(json!({"job":{"type":"string"}}), &[]),
    json!({"jobs":[],"runs":[]}),
    get_job_status
);
async fn get_job_status(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let jobs: Vec<(String, String, String, String, String, i64, Option<String>, Option<String>, Option<String>, Option<String>)> =
        sqlx::query_as("SELECT id,name,skill,at_time,timezone,enabled,next_run_at,last_run_at,last_status,last_error FROM scheduled_jobs")
            .fetch_all(&ctx.state.db).await?;
    let runs: Vec<(String, String, String, Option<String>, String, Option<String>)> = match s_opt(&a, "job") {
        Some(j) => sqlx::query_as(
            "SELECT r.id,r.job_id,r.status,r.error,r.started_at,r.finished_at FROM job_runs r
             JOIN scheduled_jobs s ON s.id=r.job_id WHERE s.name=? OR s.id=? ORDER BY r.started_at DESC LIMIT 20")
            .bind(&j).bind(&j).fetch_all(&ctx.state.db).await?,
        None => sqlx::query_as(
            "SELECT id,job_id,status,error,started_at,finished_at FROM job_runs ORDER BY started_at DESC LIMIT 20")
            .fetch_all(&ctx.state.db).await?,
    };
    Ok(json!({
        "jobs": jobs.into_iter().map(|(id,name,skill,at,tz,en,next,last,status,err)|
            json!({"id":id,"name":name,"skill":skill,"at_time":at,"timezone":tz,"enabled":en==1,
                   "next_run_at":next,"last_run_at":last,"last_status":status,"last_error":err})).collect::<Vec<_>>(),
        "runs": runs.into_iter().map(|(id,job,status,err,started,finished)|
            json!({"run_id":id,"job_id":job,"status":status,"error":err,"started_at":started,"finished_at":finished})).collect::<Vec<_>>()
    }))
}

// ------------------------------------------------------------ F21 / F22 skills

deftool!(
    RunSkill,
    "run_skill",
    "Run an installed, enabled skill by name.",
    Permission::LocalMutation,
    600_000,
    false,
    obj(
        json!({"name":{"type":"string"},"input":{"type":"string"}}),
        &["name"]
    ),
    json!({"status":"string","output":"string"}),
    run_skill
);
async fn run_skill(ctx: &ToolCtx, a: Value) -> Result<Value> {
    crate::skills::run(
        &ctx.state,
        &s(&a, "name")?,
        s_opt(&a, "input").as_deref(),
        ctx.task_id.as_deref(),
    )
    .await
}

deftool!(
    CreateSkillDraft,
    "create_skill_draft",
    "U11/F21: turn this task's recorded tool-call trace into a reviewable skill DRAFT. \
     Drafts are written disabled and must be inspected and enabled by the user.",
    Permission::LocalMutation,
    20_000,
    false,
    obj(
        json!({"name":{"type":"string","description":"kebab-case skill name"},"description":{"type":"string"},
        "task_id":{"type":"string","description":"task whose trace to convert; defaults to the current task"}}),
        &["name", "description"]
    ),
    json!({"skill":"string","enabled":false,"path":"string"}),
    create_skill_draft
);
async fn create_skill_draft(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let task_id = s_opt(&a, "task_id")
        .or_else(|| ctx.task_id.clone())
        .ok_or_else(|| anyhow::anyhow!("no task_id to derive a trace from"))?;
    crate::skills::draft_from_trace(
        &ctx.state,
        &s(&a, "name")?,
        &s(&a, "description")?,
        &task_id,
    )
    .await
}

// ------------------------------------------------------------ U12 / U13 / U14 / U15

deftool!(
    EstimateWorkflowCost, "estimate_workflow_cost",
    "Estimate model calls, tool calls and wall-clock time for a multi-agent plan before starting it.",
    Permission::ReadOnly, 5_000, false,
    obj(json!({"subagents":{"type":"integer","minimum":1,"maximum":5}}), &["subagents"]),
    json!({"is_estimate":true}), estimate_cost
);
async fn estimate_cost(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let k = n(&a, "subagents", 1).clamp(1, 5) as usize;
    Ok(agents::estimate_cost(&ctx.state.cfg().await, k))
}

deftool!(
    DetectDuplicateWork, "detect_duplicate_work",
    "Find in-flight or recent tasks that substantially overlap, so work can be merged or cancelled.",
    Permission::ReadOnly, 10_000, false,
    obj(json!({"objective":{"type":"string"}}), &["objective"]), json!({"overlapping":[]}), dup
);
async fn dup(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let objective = s(&a, "objective")?.to_lowercase();
    let words: std::collections::HashSet<&str> = objective
        .split_whitespace()
        .filter(|w| w.len() > 3)
        .collect();
    let rows: Vec<(String, String, String, String)> = sqlx::query_as(
        "SELECT id,title,status,created_at FROM tasks WHERE status IN ('pending','running')
         OR created_at > datetime('now','-1 day') ORDER BY created_at DESC LIMIT 100",
    )
    .fetch_all(&ctx.state.db)
    .await?;
    let mut overlapping = Vec::new();
    for (id, title, status, created_at) in rows {
        let t = title.to_lowercase();
        let shared = words.iter().filter(|w| t.contains(**w)).count();
        if words.len() >= 2 && shared * 2 >= words.len() {
            overlapping.push(json!({"task_id":id,"title":title,"status":status,
                "created_at":created_at,"shared_terms":shared}));
        }
    }
    Ok(json!({"objective": objective, "overlapping": overlapping,
        "recommendation": if overlapping.is_empty() { "no obvious duplicate; proceed" }
                          else { "review these before starting; consider merging or cancelling one" }}))
}

deftool!(
    CreateDecisionRecord, "create_decision_record",
    "Record a decision with the alternatives considered, evidence, trade-offs and revisit conditions.",
    Permission::LocalMutation, 10_000, false,
    obj(json!({"title":{"type":"string"},"decision":{"type":"string"},
        "alternatives":{"type":"array","items":{"type":"string"}},
        "evidence":{"type":"array","items":{"type":"string"}},
        "tradeoffs":{"type":"string"},"revisit_when":{"type":"string"}}), &["title","decision"]),
    json!({"memory_id":"string"}), decision_record
);
async fn decision_record(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let title = s(&a, "title")?;
    let body = format!(
        "DECISION: {}\nChose: {}\nAlternatives: {}\nEvidence: {}\nTrade-offs: {}\nRevisit when: {}",
        title,
        s(&a, "decision")?,
        str_list(&a, "alternatives").join("; "),
        str_list(&a, "evidence").join("; "),
        s_opt(&a, "tradeoffs").unwrap_or_else(|| "not stated".into()),
        s_opt(&a, "revisit_when").unwrap_or_else(|| "not stated".into()),
    );
    let saved = crate::memory::upsert(
        &ctx.state,
        &json!({"content": body, "category": "project", "source": format!("decision record: {title}"),
                "provenance": "task_result", "tags": ["decision"]}),
        true,
    )
    .await?;
    Ok(json!({"memory_id": saved["id"], "title": title, "recorded": true}))
}

deftool!(
    AuditMemoryConflicts, "audit_memory_conflicts",
    "Find contradictory stored memories and surface them for the user to resolve. Never resolves them automatically.",
    Permission::ReadOnly, 10_000, false, obj(json!({}), &[]), json!({"conflicts":[]}), conflicts
);
async fn conflicts(ctx: &ToolCtx, _a: Value) -> Result<Value> {
    let c = crate::memory::conflicts(&ctx.state).await?;
    Ok(json!({"count": c.len(), "conflicts": c,
        "instruction": "Present each pair and ask the user which to keep. Do not delete either on your own."}))
}

// ------------------------------------------------------------ U17 / U23 / U26

deftool!(
    SafeActionPreview, "safe_action_preview",
    "Produce a human-readable preview of what an action would change, plus whether it is reversible, \
     before anything is executed.",
    Permission::ReadOnly, 10_000, false,
    obj(json!({"action":{"type":"string"},"tool":{"type":"string"},"args":{"type":"object"}}), &["action"]),
    json!({"preview":"string","reversible":"bool"}), preview
);
async fn preview(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let action = s(&a, "action")?;
    let tool_name = s_opt(&a, "tool");
    let def = tool_name
        .as_deref()
        .and_then(|t| ctx.state.registry.get(t))
        .map(|t| t.def());
    let (permission, network) = def
        .as_ref()
        .map(|d| (d.permission.as_str(), d.network))
        .unwrap_or(("unknown", false));
    let reversible = matches!(permission, "read_only")
        || tool_name.as_deref().is_some_and(|t| {
            matches!(
                t,
                "create_task" | "propose_memory" | "update_memory" | "update_task"
            )
        });
    Ok(json!({
        "action": action, "tool": tool_name, "permission": permission,
        "leaves_this_machine": network,
        "reversible": reversible,
        "rollback": if reversible { "state change can be undone by an inverse update" }
                    else { "no verified rollback mechanism; treat as irreversible" },
        "args_echo": a.get("args").cloned().unwrap_or(json!({})),
        "preview": format!("Would run `{}` with permission `{permission}`. {}",
            tool_name.clone().unwrap_or_else(|| "(no tool named)".into()),
            if reversible { "Reversible." } else { "NOT reversible - request approval first." }),
        "next_step": if reversible { "proceed" } else { "call request_approval before executing" }
    }))
}

deftool!(
    PrivacyImpactPreview,
    "privacy_impact_preview",
    "Before sending local content to a hosted model, report which categories of information would \
     leave this machine so the user can redact or cancel.",
    Permission::ReadOnly,
    10_000,
    false,
    obj(json!({"content":{"type":"string"}}), &["content"]),
    json!({"categories":[],"destination":"string"}),
    privacy
);
async fn privacy(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let content = s(&a, "content")?;
    let cfg = ctx.state.cfg().await;
    let router = ctx.state.router.read().await.clone();
    let route = router.select(&cfg.mode).ok();
    let mut categories = Vec::new();
    let checks: &[(&str, fn(&str) -> bool)] = &[
        ("email_address", |c| c.contains('@') && c.contains('.')),
        ("url", |c| c.contains("http://") || c.contains("https://")),
        ("absolute_file_path", |c| {
            c.contains("/home/") || c.contains("/Users/")
        }),
        ("possible_credential", |c| crate::util::redact(c) != c),
        ("phone_number", |c| {
            c.chars()
                .collect::<Vec<_>>()
                .windows(10)
                .any(|w| w.iter().filter(|c| c.is_ascii_digit()).count() >= 9)
        }),
    ];
    for (name, f) in checks {
        if f(&content) {
            categories.push(*name);
        }
    }
    let hosted = route.as_ref().is_some_and(|(p, _)| p.requires_network());
    Ok(json!({
        "destination": route.as_ref().map(|(_, r)| format!("{} ({})", r.provider, r.model)),
        "leaves_this_machine": hosted,
        "detected_categories": categories,
        "bytes": content.len(),
        "redacted_preview": truncate(&crate::util::redact(&content), 2000),
        "policy": {"confirm_hosted_uploads": cfg.permissions.confirm_hosted_uploads, "mode": cfg.mode},
        "next_step": if hosted && (cfg.permissions.confirm_hosted_uploads || !categories.is_empty()) {
            "call request_approval, offering the redacted version, before sending this to the hosted model"
        } else { "no hosted transmission is implied by this content" }
    }))
}

deftool!(
    RunAdversarialReview, "run_adversarial_review",
    "U10: have a restricted verifier subagent attack a plan, patch or report and return concrete weaknesses.",
    Permission::ReadOnly, 300_000, false,
    obj(json!({"subject":{"type":"string","description":"the plan, patch or report text to attack"}}), &["subject"]),
    json!({"findings":"string"}), adversarial
);
async fn adversarial(ctx: &ToolCtx, a: Value) -> Result<Value> {
    anyhow::ensure!(
        ctx.depth == 0,
        "adversarial review cannot be launched from inside a subagent"
    );
    let task_id = ctx
        .task_id
        .clone()
        .ok_or_else(|| anyhow::anyhow!("needs a parent task"))?;
    let subject = s(&a, "subject")?;
    let cancel = ctx
        .state
        .cancels
        .read()
        .await
        .get(&task_id)
        .cloned()
        .unwrap_or_default();
    let results = agents::fan_out(
        &ctx.state,
        &task_id,
        vec![SubagentSpec {
            role: "verifier".into(),
            objective: format!(
                "Attack the following. List concrete weaknesses, missing evidence, unsupported claims and \
                 counterexamples. Be specific; do not manufacture disagreement.\n\n{}",
                truncate(&subject, 20_000)
            ),
            allowed_tools: vec![],
            output_schema: Some(json!({"weaknesses":["string"],"unsupported_claims":["string"],"verdict":"string"})),
            time_budget_secs: None,
        }],
        ctx.allowed.clone(),
        cancel,
    )
    .await?;
    Ok(json!({"review": results.into_iter().next().unwrap_or(json!({"status":"failed"}))}))
}

deftool!(
    ScheduledWorkflowRecovery, "scheduled_workflow_recovery",
    "U22: detect missed or interrupted scheduled runs, say whether a rerun is safe, and ask for approval when it is not.",
    Permission::ReadOnly, 10_000, false, obj(json!({}), &[]), json!({"missed":[]}), recovery
);
async fn recovery(ctx: &ToolCtx, _a: Value) -> Result<Value> {
    crate::scheduler::recovery_report(&ctx.state).await
}

deftool!(
    PersonalAutomationReadiness, "personal_automation_readiness",
    "U26: assess whether a workflow is safe to automate unattended, from permissions, reversibility and verification.",
    Permission::ReadOnly, 10_000, false,
    obj(json!({"workflow":{"type":"string"},"tools":{"type":"array","items":{"type":"string"}}}), &["workflow","tools"]),
    json!({"recommendation":"string","factors":{}}), readiness
);
async fn readiness(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let tools_req = str_list(&a, "tools");
    let mut unknown = Vec::new();
    let mut highest = "read_only";
    let mut network = false;
    for t in &tools_req {
        match ctx.state.registry.get(t) {
            Some(tool) => {
                let d = tool.def();
                network |= d.network;
                let rank = |p: &str| match p {
                    "read_only" => 0,
                    "local_mutation" => 1,
                    "network" => 2,
                    "external_side_effect" => 3,
                    _ => 4,
                };
                if rank(d.permission.as_str()) > rank(highest) {
                    highest = d.permission.as_str();
                }
            }
            None => unknown.push(t.clone()),
        }
    }
    let verifiable = tools_req.iter().any(|t| {
        matches!(
            t.as_str(),
            "run_project_tests" | "run_build" | "verify_task_completion" | "get_git_status"
        )
    });
    let recommendation = if !unknown.is_empty() {
        "not_ready: unknown tools requested"
    } else if matches!(highest, "external_side_effect" | "privileged") {
        "manual_approval_required: the workflow has outward-facing or privileged effects"
    } else if !verifiable {
        "automate_with_reporting: safe enough, but add a verification step before it runs unattended"
    } else {
        "ready_to_automate"
    };
    Ok(json!({
        "workflow": s(&a, "workflow")?,
        "factors": {"highest_permission": highest, "leaves_machine": network,
                    "has_verification_step": verifiable, "unknown_tools": unknown},
        "recommendation": recommendation
    }))
}
