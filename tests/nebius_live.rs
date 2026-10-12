//! Live integration against Nebius Token Factory and the hosted NVIDIA model.
//!
//! These tests make real network calls and cost tokens, so they are `#[ignore]`d
//! by default. Run them explicitly once `NEBIUS_API_KEY` is set:
//!
//! ```bash
//! cargo test --test nebius_live -- --ignored --nocapture
//! ```
//!
//! Each test skips with a clear message rather than failing when the key is
//! absent, so the default suite stays green and offline.
use friday::models::{ModelProvider, ModelRequest, Msg, OpenAiCompatible};
use serde_json::json;

fn provider() -> Option<OpenAiCompatible> {
    let key = std::env::var("NEBIUS_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())?;
    let cfg = friday::config::Config::default();
    OpenAiCompatible::new(
        "nebius",
        &std::env::var("NEBIUS_BASE_URL").unwrap_or(cfg.model.nebius_base_url),
        &std::env::var("NEBIUS_MODEL").unwrap_or(cfg.model.nebius_model),
        Some(key),
        60,
        true,
    )
    .ok()
}

macro_rules! skip_without_key {
    () => {
        match provider() {
            Some(p) => p,
            None => {
                eprintln!("skipping: NEBIUS_API_KEY is not set");
                return;
            }
        }
    };
}

#[tokio::test]
#[ignore = "makes a real Nebius Token Factory call"]
async fn the_endpoint_is_reachable_and_the_model_is_available() {
    let p = skip_without_key!();
    assert!(
        p.healthy().await,
        "GET {}/models failed - check the key and that the base URL matches your region \
         (eu: api.tokenfactory.nebius.com, us: api.tokenfactory.us-central1.nebius.com)",
        p.base_url
    );
    eprintln!("reachable: {} / {}", p.base_url, p.model);
}

#[tokio::test]
#[ignore = "makes a real Nebius Token Factory call"]
async fn the_hosted_model_completes_a_plain_prompt() {
    let p = skip_without_key!();
    let resp = p
        .generate(ModelRequest {
            messages: vec![
                Msg::new("system", "Answer with a single word."),
                Msg::new("user", "What is the capital of France?"),
            ],
            tools: vec![],
            max_tokens: 32,
            temperature: 0.0,
        })
        .await
        .expect("completion failed");

    eprintln!(
        "model={} latency={}ms content={:?}",
        resp.model, resp.latency_ms, resp.content
    );
    assert!(
        !resp.content.trim().is_empty(),
        "the model returned no content"
    );
    assert!(
        resp.content.to_lowercase().contains("paris"),
        "unexpected answer: {}",
        resp.content
    );
    assert!(
        resp.usage.completion_tokens > 0,
        "no token usage was reported"
    );
    assert!(resp.latency_ms > 0);
}

#[tokio::test]
#[ignore = "makes a real Nebius Token Factory call"]
async fn the_hosted_model_emits_a_tool_call_our_parser_understands() {
    let p = skip_without_key!();
    // This is the capability the whole agent loop depends on: the model must
    // return a structured tool call that `parse_completion` can decode.
    let resp = p
        .generate(ModelRequest {
            messages: vec![
                Msg::new(
                    "system",
                    "You are a tool-using assistant. You do not know the current time; \
                     you must call a tool to find it.",
                ),
                Msg::new("user", "What time is it right now in Asia/Kolkata?"),
            ],
            tools: vec![json!({
                "type": "function",
                "function": {
                    "name": "get_current_time",
                    "description": "Current date and time from the system clock in a given timezone.",
                    "parameters": {
                        "type": "object",
                        "properties": {
                            "timezone": {"type": "string", "description": "IANA timezone name"}
                        },
                        "required": ["timezone"]
                    }
                }
            })],
            max_tokens: 256,
            temperature: 0.0,
        })
        .await
        .expect("completion failed");

    eprintln!(
        "tool_calls={:?} content={:?}",
        resp.tool_calls, resp.content
    );
    assert_eq!(resp.tool_calls.len(), 1, "expected exactly one tool call");
    let call = &resp.tool_calls[0];
    assert_eq!(call.name, "get_current_time");
    assert!(
        !call.id.is_empty(),
        "a tool call needs an id for result pairing"
    );
    assert!(
        call.arguments
            .get("timezone")
            .and_then(|t| t.as_str())
            .is_some(),
        "arguments did not decode into an object with `timezone`: {}",
        call.arguments
    );
}

/// The end-to-end claim the hackathon submission rests on: the hosted NVIDIA
/// model drives the real agent loop, calls a real registered tool, and uses
/// its result — with the evidence recorded.
#[tokio::test]
#[ignore = "makes real Nebius Token Factory calls"]
async fn the_hosted_model_drives_the_real_agent_loop() {
    if provider().is_none() {
        eprintln!("skipping: NEBIUS_API_KEY is not set");
        return;
    }
    let (st, _mock) = friday::test_state().await.unwrap();
    // Replace the mock router with the real hosted provider.
    st.config.write().await.mode = "online".into();
    *st.router.write().await = std::sync::Arc::new(friday::models::Router {
        hosted: Some(std::sync::Arc::new(provider().unwrap())),
        ..Default::default()
    });

    let out = friday::agents::handle_message(
        &st,
        "live-1",
        "Use your tools to tell me the current date in the Asia/Kolkata timezone. \
         Call get_current_time; do not guess.",
    )
    .await
    .expect("agent run failed");

    eprintln!(
        "status={} iterations={} tools={}",
        out.status, out.iterations, out.tool_calls
    );
    eprintln!("---\n{}\n---", out.content);

    assert_eq!(out.status, "completed", "{}", out.content);
    assert!(out.tool_calls >= 1, "the model never called a tool");
    assert!(
        out.content.contains("**Evidence**"),
        "the evidence footer is missing"
    );

    // The tool really executed, against the real system clock.
    let calls: Vec<(String, i64)> =
        sqlx::query_as("SELECT tool, ok FROM tool_calls WHERE task_id=?")
            .bind(&out.task_id)
            .fetch_all(&st.db)
            .await
            .unwrap();
    assert!(
        calls
            .iter()
            .any(|(t, ok)| t == "get_current_time" && *ok == 1),
        "{calls:?}"
    );

    // And the evidence layer agrees the result is verified.
    let verdict = friday::evidence::verify(&st, &out.task_id, Some("I checked the current time"))
        .await
        .unwrap();
    assert_eq!(verdict["verified"], json!(true), "{verdict}");

    // Token usage is accounted for.
    assert!(
        out.prompt_tokens > 0 && out.completion_tokens > 0,
        "no usage reported"
    );
}

/// Bounded multi-agent orchestration against the real model.
#[tokio::test]
#[ignore = "makes real Nebius Token Factory calls (3 subagents)"]
async fn three_specialists_run_concurrently_on_the_hosted_model() {
    if provider().is_none() {
        eprintln!("skipping: NEBIUS_API_KEY is not set");
        return;
    }
    let (st, _mock) = friday::test_state().await.unwrap();
    st.config.write().await.mode = "online".into();
    *st.router.write().await = std::sync::Arc::new(friday::models::Router {
        hosted: Some(std::sync::Arc::new(provider().unwrap())),
        ..Default::default()
    });
    sqlx::query(
        "INSERT INTO tasks (id,title,description,kind,status,created_at,updated_at)
         VALUES ('live-parent','parent','','user','running',?,?)",
    )
    .bind(friday::util::now())
    .bind(friday::util::now())
    .execute(&st.db)
    .await
    .unwrap();

    let spec = |role: &str, objective: &str| friday::agents::SubagentSpec {
        role: role.into(),
        objective: objective.into(),
        allowed_tools: vec![],
        output_schema: None,
        time_budget_secs: Some(90),
    };

    let started = std::time::Instant::now();
    let results = friday::agents::fan_out(
        &st,
        "live-parent",
        vec![
            spec(
                "planner",
                "Break 'add OAuth to a Rust web service' into at most 5 ordered steps.",
            ),
            spec(
                "summarizer",
                "In two sentences, say what a write-ahead log is for.",
            ),
            spec(
                "verifier",
                "Name two concrete weaknesses of storing OAuth refresh tokens in a config file.",
            ),
        ],
        None,
        tokio_util::sync::CancellationToken::new(),
    )
    .await
    .expect("fan-out failed");

    eprintln!("3 subagents in {:?}", started.elapsed());
    for r in &results {
        eprintln!("[{}] {} -> {}", r["role"], r["status"], r["result"]);
    }
    assert_eq!(results.len(), 3);
    assert!(
        results.iter().all(|r| r["status"] == json!("completed")),
        "a subagent did not complete: {results:?}"
    );
    assert!(
        results
            .iter()
            .all(|r| !r["result"].as_str().unwrap_or("").trim().is_empty()),
        "a subagent returned no content"
    );
}
