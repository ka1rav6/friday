//! Acceptance Test D: bounded parent/child orchestration — concurrency limits,
//! typed results, grant narrowing, and cancellation propagation.
use friday::agents::{self, SubagentSpec};
use friday::models::MockProvider;
use serde_json::json;
use tokio_util::sync::CancellationToken;

fn spec(role: &str, objective: &str) -> SubagentSpec {
    SubagentSpec {
        role: role.into(),
        objective: objective.into(),
        allowed_tools: vec![],
        output_schema: None,
        time_budget_secs: Some(20),
    }
}

async fn parent_task(st: &friday::state::AppState, id: &str) {
    sqlx::query(
        "INSERT INTO tasks (id,title,description,kind,status,created_at,updated_at)
         VALUES (?,?,'','user','running',?,?)",
    )
    .bind(id)
    .bind("parent")
    .bind(friday::util::now())
    .bind(friday::util::now())
    .execute(&st.db)
    .await
    .unwrap();
}

#[tokio::test]
async fn three_specialists_run_independently_and_return_structured_results() {
    let (st, mock) = friday::test_state().await.unwrap();
    parent_task(&st, "parent-1").await;
    for _ in 0..3 {
        mock.push(MockProvider::text("subtask finding"));
    }

    let results = agents::fan_out(
        &st,
        "parent-1",
        vec![
            spec("researcher", "find primary sources on io_uring"),
            spec("summarizer", "summarize the saved notes"),
            spec("planner", "break the migration into steps"),
        ],
        None,
        CancellationToken::new(),
    )
    .await
    .unwrap();

    assert_eq!(results.len(), 3);
    for r in &results {
        assert_eq!(r["status"], json!("completed"), "{r}");
        assert!(r["role"].is_string());
        assert!(r["task_id"].is_string());
        assert!(r["result"].as_str().unwrap().contains("subtask finding"));
    }

    // Each child is a durable task row parented to the caller.
    let children: Vec<(String, String, Option<String>)> =
        sqlx::query_as("SELECT id,status,role FROM tasks WHERE parent_id='parent-1'")
            .fetch_all(&st.db)
            .await
            .unwrap();
    assert_eq!(children.len(), 3);
    assert!(children.iter().all(|c| c.1 == "completed"));
    let mut roles: Vec<String> = children.iter().filter_map(|c| c.2.clone()).collect();
    roles.sort();
    assert_eq!(roles, vec!["planner", "researcher", "summarizer"]);

    // Every child is recorded as an agent run under the same parent task.
    let runs: i64 =
        sqlx::query_scalar("SELECT COUNT(*) FROM agent_runs WHERE parent_task_id='parent-1'")
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert_eq!(runs, 3);
}

#[tokio::test]
async fn concurrency_never_exceeds_the_configured_limit() {
    let (st, mock) = friday::test_state().await.unwrap();
    st.config.write().await.agents.max_concurrent_subagents = 2;
    parent_task(&st, "parent-2").await;
    for _ in 0..4 {
        mock.push(MockProvider::text("ok"));
    }

    let before = st.subagent_sem.available_permits();
    let results = agents::fan_out(
        &st,
        "parent-2",
        (0..4)
            .map(|i| spec("summarizer", &format!("chunk {i}")))
            .collect(),
        None,
        CancellationToken::new(),
    )
    .await
    .unwrap();
    assert_eq!(results.len(), 4);
    // Permits are released, so the global budget is not leaked.
    assert_eq!(st.subagent_sem.available_permits(), before);
}

#[tokio::test]
async fn more_subagents_than_the_hard_maximum_is_refused() {
    let (st, _) = friday::test_state().await.unwrap();
    parent_task(&st, "parent-3").await;
    let err = agents::fan_out(
        &st,
        "parent-3",
        (0..6)
            .map(|i| spec("summarizer", &format!("chunk {i}")))
            .collect(),
        None,
        CancellationToken::new(),
    )
    .await
    .unwrap_err();
    assert!(format!("{err:#}").contains("hard_max_subagents"));
}

#[tokio::test]
async fn an_empty_fan_out_is_refused() {
    let (st, _) = friday::test_state().await.unwrap();
    assert!(
        agents::fan_out(&st, "p", vec![], None, CancellationToken::new())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn a_child_grant_is_an_intersection_and_can_never_widen() {
    let (st, mock) = friday::test_state().await.unwrap();
    parent_task(&st, "parent-4").await;
    mock.push(MockProvider::text("done"));

    // The child asks for privileged tools its role never had.
    let out = agents::run_subagent(
        &st,
        "parent-4",
        SubagentSpec {
            role: "summarizer".into(),
            objective: "summarize".into(),
            allowed_tools: vec![
                "read_local_file".into(), // in the summarizer role default
                "delete_memory".into(),   // never granted
                "spawn_subagent".into(),  // delegation is forbidden
                "search_web".into(),      // not in this role
            ],
            output_schema: None,
            time_budget_secs: Some(20),
        },
        None,
        CancellationToken::new(),
    )
    .await
    .unwrap();

    let granted: String = sqlx::query_scalar("SELECT allowed_tools FROM agent_runs WHERE id=?")
        .bind(&out.run_id)
        .fetch_one(&st.db)
        .await
        .unwrap();
    let granted: Vec<String> = serde_json::from_str(&granted).unwrap();
    assert!(granted.contains(&"read_local_file".to_string()));
    assert!(!granted.contains(&"delete_memory".to_string()));
    assert!(!granted.contains(&"spawn_subagent".to_string()));
    assert!(
        !granted.contains(&"search_web".to_string()),
        "a child widened its own grant: {granted:?}"
    );
}

#[tokio::test]
async fn a_parent_grant_further_narrows_its_children() {
    let (st, mock) = friday::test_state().await.unwrap();
    parent_task(&st, "parent-5").await;
    mock.push(MockProvider::text("done"));
    let parent_grant: std::collections::HashSet<String> =
        ["search_knowledge".to_string()].into_iter().collect();

    let out = agents::run_subagent(
        &st,
        "parent-5",
        spec("summarizer", "summarize"),
        Some(parent_grant),
        CancellationToken::new(),
    )
    .await
    .unwrap();
    let granted: Vec<String> = serde_json::from_str(
        &sqlx::query_scalar::<_, String>("SELECT allowed_tools FROM agent_runs WHERE id=?")
            .bind(&out.run_id)
            .fetch_one(&st.db)
            .await
            .unwrap(),
    )
    .unwrap();
    assert!(granted.contains(&"search_knowledge".to_string()));
    assert!(
        !granted.contains(&"read_local_file".to_string()),
        "the parent's grant must bound the child: {granted:?}"
    );
}

#[tokio::test]
async fn cancellation_propagates_from_parent_to_children() {
    let (st, mock) = friday::test_state().await.unwrap();
    parent_task(&st, "parent-6").await;
    for _ in 0..3 {
        mock.push(MockProvider::text("never used"));
    }
    let cancel = CancellationToken::new();
    cancel.cancel();

    let results = agents::fan_out(
        &st,
        "parent-6",
        vec![
            spec("researcher", "a"),
            spec("summarizer", "b"),
            spec("planner", "c"),
        ],
        None,
        cancel,
    )
    .await
    .unwrap();
    assert_eq!(results.len(), 3);
    assert!(
        results.iter().all(|r| r["status"] == json!("cancelled")),
        "every child must observe the parent's cancellation: {results:?}"
    );
    let cancelled: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tasks WHERE parent_id='parent-6' AND status='cancelled'",
    )
    .fetch_one(&st.db)
    .await
    .unwrap();
    assert_eq!(cancelled, 3);
}

#[tokio::test]
async fn cancel_task_marks_the_whole_subtree() {
    let (st, _) = friday::test_state().await.unwrap();
    parent_task(&st, "parent-7").await;
    for child in ["c1", "c2"] {
        sqlx::query(
            "INSERT INTO tasks (id,parent_id,title,description,kind,status,created_at,updated_at)
             VALUES (?,'parent-7',?,'','subagent','running',?,?)",
        )
        .bind(child)
        .bind(child)
        .bind(friday::util::now())
        .bind(friday::util::now())
        .execute(&st.db)
        .await
        .unwrap();
    }
    let ctx = friday::tools::ToolCtx::root(st.clone(), Some("parent-7".into()));
    let out = friday::tools::execute(&ctx, "cancel_task", json!({"task_id":"parent-7"})).await;
    assert!(out.ok);
    let open: i64 = sqlx::query_scalar(
        "SELECT COUNT(*) FROM tasks WHERE (id='parent-7' OR parent_id='parent-7') AND status != 'cancelled'")
        .fetch_one(&st.db).await.unwrap();
    assert_eq!(open, 0);
}

#[tokio::test]
async fn spawn_subagent_reports_an_estimate_and_the_actual_cost() {
    let (st, mock) = friday::test_state().await.unwrap();
    parent_task(&st, "parent-8").await;
    for _ in 0..2 {
        mock.push(MockProvider::text("finding"));
    }
    let ctx = friday::tools::ToolCtx::root(st.clone(), Some("parent-8".into()));
    let out = friday::tools::execute(
        &ctx,
        "spawn_subagent",
        json!({"subagents":[
            {"role":"researcher","objective":"compare A"},
            {"role":"verifier","objective":"attack the comparison"}]}),
    )
    .await;
    assert!(out.ok, "{:?}", out.value);
    assert_eq!(out.value["estimate"]["is_estimate"], json!(true));
    assert_eq!(out.value["actual"]["subagents"], json!(2));
    assert_eq!(out.value["actual"]["completed"], json!(2));

    // The fan-out is itself recorded as evidence on the parent task.
    let kinds: Vec<(String,)> =
        sqlx::query_as("SELECT kind FROM evidence WHERE task_id='parent-8'")
            .fetch_all(&st.db)
            .await
            .unwrap();
    assert!(kinds.iter().any(|k| k.0 == "subagent_fanout"));
}
