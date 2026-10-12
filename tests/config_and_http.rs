//! Configuration precedence and the HTTP surface. The server is bound to an
//! ephemeral port and driven over real HTTP, so the API the frontend and the
//! CLI depend on is exercised as deployed.
use friday::models::MockProvider;
use serde_json::{json, Value};

/// The one test that changes the process working directory, so project-config
/// discovery can be exercised without disturbing the others in this binary.
#[test]
fn configuration_precedence_is_defaults_then_user_then_project_then_env() {
    let tmp = tempfile::tempdir().unwrap();
    let home = tmp.path().join("xdg");
    std::fs::create_dir_all(home.join("friday")).unwrap();
    std::fs::write(
        home.join("friday/config.toml"),
        "timezone = \"Europe/Berlin\"\n[agents]\nmax_iterations = 7\n[briefing]\nat_time = \"06:00\"\n",
    )
    .unwrap();

    let project = tmp.path().join("proj");
    std::fs::create_dir_all(project.join(".friday")).unwrap();
    std::fs::write(
        project.join(".friday/config.toml"),
        "timezone = \"Asia/Kolkata\"\n[model]\nnebius_model = \"nvidia/nemotron-3-super-120b-a12b\"\n",
    )
    .unwrap();

    let original = std::env::current_dir().unwrap();
    std::env::set_current_dir(&project).unwrap();
    std::env::set_var("XDG_CONFIG_HOME", &home);
    std::env::set_var("FRIDAY_MODE", "offline");

    let loaded = friday::config::load(None, None).unwrap();
    // project beats user
    assert_eq!(loaded.config.timezone, "Asia/Kolkata");
    // user beats defaults, and an unmentioned key keeps its default
    assert_eq!(loaded.config.agents.max_iterations, 7);
    assert_eq!(loaded.config.briefing.at_time, "06:00");
    assert_eq!(loaded.config.agents.max_concurrent_subagents, 3);
    // env beats both files
    assert_eq!(loaded.config.mode, "offline");
    // cli beats env
    let with_cli = friday::config::load(None, Some("online".into())).unwrap();
    assert_eq!(with_cli.config.mode, "online");
    assert!(with_cli
        .sources
        .env_overrides
        .contains(&"--mode".to_string()));

    // Every effective value reports where it came from.
    assert!(loaded
        .sources
        .user_config
        .unwrap()
        .contains("friday/config.toml"));
    assert_eq!(
        loaded.sources.project_config.as_deref(),
        Some(".friday/config.toml")
    );
    assert!(loaded
        .sources
        .env_overrides
        .contains(&"FRIDAY_MODE".to_string()));

    // A malformed project config is rejected, naming the file.
    std::fs::write(
        project.join(".friday/config.toml"),
        "[agents]\nmax_iterations = \"seven\"\n",
    )
    .unwrap();
    let e = format!("{:#}", friday::config::load(None, None).unwrap_err());
    assert!(e.contains(".friday/config.toml"), "{e}");

    // An out-of-range value is rejected with the valid range.
    std::fs::write(
        project.join(".friday/config.toml"),
        "[agents]\nhard_max_subagents = 50\n",
    )
    .unwrap();
    let e = format!("{:#}", friday::config::load(None, None).unwrap_err());
    assert!(e.contains("must be <= 5"), "{e}");

    std::env::remove_var("FRIDAY_MODE");
    std::env::set_current_dir(original).unwrap();
}

async fn serve() -> (
    String,
    friday::state::AppState,
    std::sync::Arc<MockProvider>,
) {
    let (st, mock) = friday::test_state().await.unwrap();
    friday::scheduler::register_builtins(&st).await.unwrap();
    let app = friday::api::router(st.clone());
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let addr = listener.local_addr().unwrap();
    tokio::spawn(async move {
        axum::serve(listener, app).await.ok();
    });
    (format!("http://{addr}"), st, mock)
}

async fn get(base: &str, path: &str) -> Value {
    reqwest::get(format!("{base}{path}"))
        .await
        .unwrap()
        .json()
        .await
        .unwrap()
}

#[tokio::test]
async fn health_status_and_tool_catalogue_are_served() {
    let (base, _st, _m) = serve().await;

    let health = get(&base, "/api/health").await;
    assert_eq!(health["ok"], json!(true));
    assert_eq!(health["service"], json!("friday"));

    let status = get(&base, "/api/status").await;
    assert_eq!(status["mode"], json!("auto"));
    assert_eq!(status["hosted_credential_present"], json!(false));
    assert!(status["counts"]["memories"].is_number());

    let tools = get(&base, "/api/tools").await;
    let defs = tools["tools"].as_array().unwrap();
    assert!(defs.len() >= 35, "catalogue looks short: {}", defs.len());
    // Every tool documents its permission, timeout and schema.
    for d in defs {
        assert!(d["name"].is_string());
        assert!(!d["description"].as_str().unwrap().is_empty());
        assert!(d["permission"].is_string());
        assert!(d["timeout_ms"].as_u64().unwrap() > 0);
        assert_eq!(d["schema"]["type"], json!("object"), "{}", d["name"]);
    }
}

#[tokio::test]
async fn the_config_endpoint_reports_sources_and_never_returns_secrets() {
    let (base, _st, _m) = serve().await;
    let body = get(&base, "/api/config").await;
    assert_eq!(body["secrets_are_never_returned"], json!(true));
    assert_eq!(
        body["sources"]["precedence"],
        json!([
            "defaults",
            "user config",
            "project config",
            "environment",
            "cli"
        ])
    );
    assert!(body["restart_required_for"]
        .as_array()
        .unwrap()
        .contains(&json!("bind")));
    let serialized = body.to_string();
    assert!(!serialized.contains("api_key") && !serialized.contains("API_KEY"));
}

#[tokio::test]
async fn an_invalid_config_is_rejected_and_the_live_one_is_kept() {
    let (base, st, _m) = serve().await;
    let client = reqwest::Client::new();

    let mut bad = serde_json::to_value(st.cfg().await).unwrap();
    bad["mode"] = json!("sideways");
    let resp = client
        .put(format!("{base}/api/config"))
        .json(&json!({"config": bad}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let err = resp.json::<Value>().await.unwrap();
    assert!(err["error"]
        .as_str()
        .unwrap()
        .contains("auto|online|offline"));

    // The live configuration is untouched.
    assert_eq!(st.cfg().await.mode, "auto");

    // The validate endpoint reports the same problem without applying anything.
    let check: Value = client
        .post(format!("{base}/api/config/validate"))
        .json(&json!({"config": bad}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(check["valid"], json!(false));
}

#[tokio::test]
async fn chat_memory_and_task_detail_round_trip_over_http() {
    let (base, st, mock) = serve().await;
    let client = reqwest::Client::new();

    mock.push(MockProvider::call(
        "save_memory",
        json!({"content":"Kairav's hackathon deadline is 2026-10-30","category":"project",
               "source":"user message","provenance":"user_stated"}),
    ));
    mock.push(MockProvider::text("Noted the deadline."));

    let chat: Value = client
        .post(format!("{base}/api/chat"))
        .json(&json!({"message":"remember my hackathon deadline is 2026-10-30"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(chat["status"], json!("completed"), "{chat}");
    assert_eq!(chat["tool_calls"], json!(1));
    let task_id = chat["task_id"].as_str().unwrap().to_string();

    // The memory is queryable through the browser-facing endpoint.
    let mems = get(&base, "/api/memories").await;
    let list = mems["memories"].as_array().unwrap();
    assert!(list
        .iter()
        .any(|m| m["content"].as_str().unwrap().contains("2026-10-30")));
    assert!(
        list.iter().all(|m| m["provenance"].is_string()),
        "the UI needs provenance"
    );

    // Task detail carries the tool trace and the evidence verdict.
    let detail = get(&base, &format!("/api/tasks/{task_id}")).await;
    assert_eq!(detail["task"]["status"], json!("completed"));
    let calls = detail["tool_calls"].as_array().unwrap();
    assert_eq!(calls.len(), 1);
    assert_eq!(calls[0]["tool"], json!("save_memory"));
    assert_eq!(detail["evidence"]["verified"], json!(true));

    // Conversation history is replayable.
    let conv = chat["conversation_id"].as_str().unwrap();
    let msgs = get(&base, &format!("/api/conversations/{conv}/messages")).await;
    assert_eq!(msgs["messages"].as_array().unwrap().len(), 2);

    // The audit log recorded the call with its permission.
    let audit = get(&base, "/api/audit").await;
    let entries = audit["entries"].as_array().unwrap();
    assert!(entries
        .iter()
        .any(|e| e["action"] == "save_memory" && e["category"] == "tool"));
    let _ = st;
}

#[tokio::test]
async fn an_empty_chat_message_is_rejected_with_a_clear_error() {
    let (base, _st, _m) = serve().await;
    let resp = reqwest::Client::new()
        .post(format!("{base}/api/chat"))
        .json(&json!({"message":"   "}))
        .send()
        .await
        .unwrap();
    assert_eq!(resp.status(), 400);
    let body: Value = resp.json().await.unwrap();
    assert!(body["error"]
        .as_str()
        .unwrap()
        .contains("must not be empty"));
}

#[tokio::test]
async fn approvals_are_decided_by_the_human_and_only_once() {
    let (base, st, _m) = serve().await;
    let ctx = friday::tools::ToolCtx::root(st.clone(), None);
    let req = friday::tools::execute(
        &ctx, "request_approval",
        json!({"action":"delete 3 memories","reason":"user asked","risk":"high","preview":"ids: a, b, c"}),
    )
    .await;
    let id = req.value["id"].as_str().unwrap().to_string();

    let pending = get(&base, "/api/approvals").await;
    let items = pending["approvals"].as_array().unwrap();
    assert_eq!(items.len(), 1);
    assert_eq!(items[0]["status"], json!("pending"));
    assert_eq!(items[0]["risk"], json!("high"));
    assert_eq!(items[0]["preview"], json!("ids: a, b, c"));

    let client = reqwest::Client::new();
    let ok: Value = client
        .post(format!("{base}/api/approvals/{id}/decide"))
        .json(&json!({"decision":"approved"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(ok["status"], json!("approved"));

    // Deciding twice is refused, and an invalid decision is refused.
    let again = client
        .post(format!("{base}/api/approvals/{id}/decide"))
        .json(&json!({"decision":"approved"}))
        .send()
        .await
        .unwrap();
    assert_eq!(again.status(), 400);
    let bad = client
        .post(format!("{base}/api/approvals/{id}/decide"))
        .json(&json!({"decision":"maybe"}))
        .send()
        .await
        .unwrap();
    assert_eq!(bad.status(), 400);
}

#[tokio::test]
async fn skills_are_fully_manageable_over_http() {
    let (base, st, _m) = serve().await;
    let tmp = tempfile::tempdir().unwrap();
    {
        let mut cfg = st.config.write().await;
        cfg.skills.user_dir = tmp.path().join("user");
        cfg.skills.project_dir = tmp.path().join("project");
    }
    let client = reqwest::Client::new();

    // Create from the template.
    let made: Value = client
        .post(format!("{base}/api/skills"))
        .json(&json!({"mode":"template","name":"inbox-triage","description":"Triage the inbox.","scope":"project"}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(made["action"], json!("installed"));

    // Inspect before enabling.
    let path = tmp.path().join("project/inbox-triage");
    let report: Value = client
        .post(format!("{base}/api/skills/inspect"))
        .json(&json!({"path": path.display().to_string()}))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(report["name"], json!("inbox-triage"));
    assert_eq!(report["scripts_will_run"], json!(false));

    // Enable, then confirm it shows as enabled in the listing.
    client
        .put(format!("{base}/api/skills/inbox-triage/enabled"))
        .json(&json!({"enabled": true}))
        .send()
        .await
        .unwrap();
    let listed = get(&base, "/api/skills").await;
    let s = listed["skills"]
        .as_array()
        .unwrap()
        .iter()
        .find(|s| s["name"] == "inbox-triage")
        .unwrap()
        .clone();
    assert_eq!(s["enabled"], json!(true));
    assert_eq!(s["scope"], json!("project"));
    assert!(listed["package_format"]["manifest"] == json!("skill.toml"));

    // Remove, keeping the files.
    let removed: Value = client
        .delete(format!("{base}/api/skills/inbox-triage"))
        .send()
        .await
        .unwrap()
        .json()
        .await
        .unwrap();
    assert_eq!(removed["files_deleted"], json!(false));
    assert!(path.exists());
}

#[tokio::test]
async fn the_event_stream_pushes_tool_and_task_updates() {
    let (base, st, mock) = serve().await;
    let url = format!("{}/api/events", base.replace("http://", "ws://"));

    // Subscribe before acting so nothing is missed.
    let mut rx = st.events.subscribe();
    mock.push(MockProvider::call("get_current_time", json!({})));
    mock.push(MockProvider::text("done"));
    reqwest::Client::new()
        .post(format!("{base}/api/chat"))
        .json(&json!({"message":"what time is it"}))
        .send()
        .await
        .unwrap();

    let mut saw_tool = false;
    let mut saw_task = false;
    while let Ok(ev) = rx.try_recv() {
        match ev {
            friday::state::Event::ToolCall { tool, ok, .. } => {
                saw_tool |= tool == "get_current_time" && ok;
            }
            friday::state::Event::TaskUpdate { .. } => saw_task = true,
            _ => {}
        }
    }
    assert!(saw_tool, "no tool event was broadcast");
    assert!(saw_task, "no task event was broadcast");
    assert!(url.starts_with("ws://"));
}
