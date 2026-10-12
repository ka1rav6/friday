//! The bounded agent loop, driven by the deterministic mock provider so the
//! control flow (tool dispatch, budgets, cancellation, evidence) is asserted
//! without a network call.
use friday::models::MockProvider;
use serde_json::json;
use tokio_util::sync::CancellationToken;

#[tokio::test]
async fn the_agent_executes_a_real_tool_and_uses_its_result() {
    let (st, mock) = friday::test_state().await.unwrap();
    mock.push(MockProvider::call(
        "get_current_time",
        json!({"timezone":"UTC"}),
    ));
    mock.push(MockProvider::text("It is currently that time."));

    let out = friday::agents::handle_message(&st, "conv-1", "what time is it?")
        .await
        .unwrap();
    assert_eq!(out.status, "completed");
    assert_eq!(out.tool_calls, 1);
    assert_eq!(
        out.iterations, 2,
        "one turn to call the tool, one to answer"
    );

    // The tool really ran, and the real system clock answered it.
    let (tool, ok): (String, i64) =
        sqlx::query_as("SELECT tool, ok FROM tool_calls WHERE task_id=?")
            .bind(&out.task_id)
            .fetch_one(&st.db)
            .await
            .unwrap();
    assert_eq!((tool.as_str(), ok), ("get_current_time", 1));

    // Proof-Carrying Actions: the answer carries its evidence.
    assert!(out.content.contains("**Evidence**"), "{}", out.content);
    assert!(out.content.contains("get_current_time"));

    // The exchange is persisted for the next session.
    let roles: Vec<(String,)> = sqlx::query_as(
        "SELECT role FROM messages WHERE conversation_id='conv-1' ORDER BY created_at",
    )
    .fetch_all(&st.db)
    .await
    .unwrap();
    assert_eq!(roles.len(), 2);
}

#[tokio::test]
async fn a_failing_tool_is_reported_not_hidden() {
    let (st, mock) = friday::test_state().await.unwrap();
    mock.push(MockProvider::call(
        "read_local_file",
        json!({"workspace":"nope","path":"x"}),
    ));
    mock.push(MockProvider::text("I could not read that file."));

    let out = friday::agents::handle_message(&st, "conv-2", "read a file")
        .await
        .unwrap();
    assert!(
        out.content.contains("(failed)"),
        "the evidence footer must disclose the failure: {}",
        out.content
    );

    let verdict = friday::evidence::verify(&st, &out.task_id, Some("I read the file"))
        .await
        .unwrap();
    assert_eq!(verdict["verified"], json!(false));
    assert!(verdict["instruction"]
        .as_str()
        .unwrap()
        .contains("Do NOT report"));
}

#[tokio::test]
async fn completion_is_not_claimable_without_successful_tool_evidence() {
    let (st, mock) = friday::test_state().await.unwrap();
    mock.push(MockProvider::text("All done, I updated the files!"));
    let out = friday::agents::handle_message(&st, "conv-3", "update the files")
        .await
        .unwrap();

    let verdict = friday::evidence::verify(&st, &out.task_id, Some("I updated the files"))
        .await
        .unwrap();
    assert_eq!(verdict["verified"], json!(false));
    let missing = verdict["missing"].as_array().unwrap();
    assert!(missing
        .iter()
        .any(|m| m.as_str().unwrap().contains("no tool call succeeded")));
    assert!(missing.iter().any(|m| m
        .as_str()
        .unwrap()
        .contains("asserts an action was performed")));
}

#[tokio::test]
async fn the_iteration_budget_stops_a_runaway_loop() {
    let (st, mock) = friday::test_state().await.unwrap();
    st.config.write().await.agents.max_iterations = 3;
    // The model never stops asking for tools.
    for _ in 0..10 {
        mock.push(MockProvider::call("get_current_time", json!({})));
    }
    let out = friday::agents::handle_message(&st, "conv-4", "loop forever")
        .await
        .unwrap();
    assert_eq!(out.status, "failed");
    assert_eq!(out.iterations, 3);
    assert!(out.content.contains("configured limit of 3 iterations"));
}

#[tokio::test]
async fn the_tool_call_budget_is_enforced_within_an_iteration() {
    let (st, mock) = friday::test_state().await.unwrap();
    {
        let mut cfg = st.config.write().await;
        cfg.agents.max_tool_calls = 2;
        cfg.agents.max_iterations = 6;
    }
    for _ in 0..6 {
        mock.push(MockProvider::call("get_current_time", json!({})));
    }
    let out = friday::agents::handle_message(&st, "conv-5", "spam tools")
        .await
        .unwrap();
    assert_eq!(out.tool_calls, 2, "budget must cap executions");
    let executed: i64 = sqlx::query_scalar("SELECT COUNT(*) FROM tool_calls WHERE task_id=?")
        .bind(&out.task_id)
        .fetch_one(&st.db)
        .await
        .unwrap();
    assert_eq!(executed, 2);
}

#[tokio::test]
async fn a_cancelled_run_stops_and_says_so() {
    let (st, mock) = friday::test_state().await.unwrap();
    mock.push(MockProvider::text("should never be reached"));
    let cancel = CancellationToken::new();
    cancel.cancel();

    let out = friday::agents::run(
        &st,
        friday::agents::RunOpts {
            task_id: "task-cancelled".into(),
            parent_task_id: None,
            role: "generalist".into(),
            objective: "do something slow".into(),
            allowed: None,
            depth: 0,
            history: vec![],
            deadline_secs: 30,
            cancel,
            output_schema: None,
        },
    )
    .await
    .unwrap();
    assert_eq!(out.status, "cancelled");
    assert_eq!(out.tool_calls, 0);

    let status: String = sqlx::query_scalar("SELECT status FROM agent_runs WHERE id=?")
        .bind(&out.run_id)
        .fetch_one(&st.db)
        .await
        .unwrap();
    assert_eq!(status, "cancelled");
}

#[tokio::test]
async fn a_provider_error_fails_honestly_instead_of_inventing_an_answer() {
    let (st, _) = friday::test_state().await.unwrap();
    // No hosted credential, no local model, and offline mode forbids the mock.
    st.config.write().await.mode = "offline".into();
    let err = friday::agents::handle_message(&st, "conv-6", "hello")
        .await
        .unwrap_err();
    let msg = format!("{err:#}");
    assert!(msg.contains("local model"), "{msg}");
}

#[tokio::test]
async fn offline_mode_hides_network_tools_from_the_model() {
    let (st, _) = friday::test_state().await.unwrap();
    let online = st.registry.schemas_for(None, false);
    let offline = st.registry.schemas_for(None, true);
    assert!(offline.len() < online.len());
    let names: Vec<&str> = offline
        .iter()
        .map(|v| v["function"]["name"].as_str().unwrap())
        .collect();
    assert!(!names.contains(&"search_web"));
    assert!(!names.contains(&"research_topic"));
    assert!(
        names.contains(&"get_research_report"),
        "saved research must stay reachable offline"
    );
}

#[tokio::test]
async fn explain_why_reports_recorded_metadata_not_speculation() {
    let (st, mock) = friday::test_state().await.unwrap();
    mock.push(MockProvider::call("get_current_time", json!({})));
    mock.push(MockProvider::text("done"));
    let out = friday::agents::handle_message(&st, "conv-7", "what time is it")
        .await
        .unwrap();

    let ctx = friday::tools::ToolCtx::root(st.clone(), None);
    let why = friday::tools::execute(&ctx, "explain_why", json!({"task_id": out.task_id})).await;
    assert!(why.ok);
    assert_eq!(why.value["agents"].as_array().unwrap().len(), 1);
    assert_eq!(
        why.value["tool_calls"][0]["tool"],
        json!("get_current_time")
    );
    assert!(why.value["note"]
        .as_str()
        .unwrap()
        .contains("not the model's internal reasoning"));
}
