//! Acceptance Tests C and F: research is saved with verifiable sources, stays
//! answerable with the network off, and online-only work fails honestly.
use friday::research::{self, Source};
use friday::tools::{self, ToolCtx};
use serde_json::json;

fn src(url: &str, title: &str) -> Source {
    Source {
        url: url.into(),
        title: title.into(),
        snippet: "evidence passage".into(),
        published: Some("2026-09-01".into()),
    }
}

async fn library() -> friday::state::AppState {
    let (st, _) = friday::test_state().await.unwrap();
    research::save_report(
        &st,
        research::NewReport {
            title: "io_uring vs epoll for high-concurrency servers",
            question: "Should a new Rust server use io_uring or epoll?",
            summary: "io_uring reduces syscall overhead; epoll remains the portable default.",
            content:
                "## Findings\nio_uring batches submissions (src 1). epoll is portable (src 2).\n",
            sources: &[
                src("https://www.kernel.org/doc/html/latest/", "Kernel docs"),
                src(
                    "https://man7.org/linux/man-pages/man7/epoll.7.html",
                    "epoll(7)",
                ),
            ],
            tags: json!(["io_uring", "networking"]),
            freshness_days: 180,
        },
    )
    .await
    .unwrap();
    st
}

#[tokio::test]
async fn a_saved_report_keeps_its_sources_and_fetch_timestamps() {
    let st = library().await;
    let found = research::search(&st, "io_uring epoll", 5).await.unwrap();
    assert_eq!(found.len(), 1, "{found:?}");
    let id = found[0]["id"].as_str().unwrap();

    let full = research::get_report(&st, id).await.unwrap();
    let sources = full["sources"].as_array().unwrap();
    assert_eq!(sources.len(), 2);
    for s in sources {
        assert!(s["url"].as_str().unwrap().starts_with("https://"));
        assert!(
            s["fetched_at"].is_string(),
            "every source needs a retrieval timestamp"
        );
    }
    assert_eq!(
        full["question"],
        json!("Should a new Rust server use io_uring or epoll?")
    );
}

#[tokio::test]
async fn saved_research_answers_questions_with_the_network_off() {
    let st = library().await;
    st.config.write().await.mode = "offline".into();
    let ctx = ToolCtx::root(st.clone(), None);

    // The live path is refused...
    let live = tools::execute(&ctx, "search_web", json!({"query":"io_uring"})).await;
    assert!(!live.ok);
    assert!(live.value["error"]
        .as_str()
        .unwrap()
        .contains("offline mode"));

    // ...but the library still answers, with its sources.
    let local = tools::execute(&ctx, "search_knowledge", json!({"query":"io_uring epoll"})).await;
    assert!(local.ok);
    let hits = local.value["research"].as_array().unwrap();
    assert_eq!(hits.len(), 1);
    assert_eq!(hits[0]["sources"].as_array().unwrap().len(), 2);
    assert!(local.value["note"]
        .as_str()
        .unwrap()
        .contains("may be out of date"));

    // And the full report is retrievable offline.
    let id = hits[0]["id"].as_str().unwrap();
    assert!(
        tools::execute(&ctx, "get_research_report", json!({"id": id}))
            .await
            .ok
    );
}

#[tokio::test]
async fn an_offline_packet_bundles_reports_with_sources_and_ages() {
    let st = library().await;
    let ctx = ToolCtx::root(st.clone(), None);
    let out = tools::execute(&ctx, "prepare_offline_packet", json!({"topic":"io_uring"})).await;
    assert!(out.ok, "{:?}", out.value);
    let md = out.value["markdown"].as_str().unwrap();
    assert!(md.contains("Offline packet: io_uring"));
    assert!(
        md.contains("https://man7.org"),
        "the packet must carry its citations"
    );
    assert!(
        md.contains("days old"),
        "the packet must state how stale it is"
    );
    assert_eq!(out.value["sources"], json!(2));

    // The packet itself is retrievable offline.
    st.config.write().await.mode = "offline".into();
    let packet_id = out.value["packet_id"].as_str().unwrap();
    assert!(
        tools::execute(&ctx, "get_research_report", json!({"id": packet_id}))
            .await
            .ok
    );
}

#[tokio::test]
async fn preparing_a_packet_for_an_unresearched_topic_fails_honestly() {
    let (st, _) = friday::test_state().await.unwrap();
    let ctx = ToolCtx::root(st.clone(), None);
    let out = tools::execute(
        &ctx,
        "prepare_offline_packet",
        json!({"topic":"quantum basket weaving"}),
    )
    .await;
    assert!(!out.ok);
    assert!(out.value["error"]
        .as_str()
        .unwrap()
        .contains("Run research_topic while online first"));
}

#[tokio::test]
async fn stale_reports_are_flagged_by_their_freshness_policy() {
    let (st, _) = friday::test_state().await.unwrap();
    let id = research::save_report(
        &st,
        research::NewReport {
            title: "Fast-moving topic",
            question: "q",
            summary: "s",
            content: "c",
            sources: &[src("https://example.com/a", "a")],
            tags: json!([]),
            freshness_days: 30,
        },
    )
    .await
    .unwrap();
    sqlx::query("UPDATE research_documents SET created_at=? WHERE id=?")
        .bind((chrono::Utc::now() - chrono::Duration::days(90)).to_rfc3339())
        .bind(&id)
        .execute(&st.db)
        .await
        .unwrap();

    let stale = research::freshness_check(&st).await.unwrap();
    assert_eq!(stale.len(), 1);
    assert!(stale[0]["age_days"].as_i64().unwrap() > 30);

    let hits = research::search(&st, "Fast-moving", 5).await.unwrap();
    assert_eq!(
        hits[0]["stale"],
        json!(true),
        "search results must disclose staleness"
    );
}

#[tokio::test]
async fn web_search_without_a_configured_provider_refuses_rather_than_inventing() {
    let (st, _) = friday::test_state().await.unwrap();
    // No TAVILY_API_KEY in the test environment.
    if std::env::var("TAVILY_API_KEY").is_ok() {
        return;
    }
    let ctx = ToolCtx::root(st.clone(), None);
    let out = tools::execute(&ctx, "search_web", json!({"query":"anything"})).await;
    assert!(!out.ok);
    let e = out.value["error"].as_str().unwrap();
    assert!(e.contains("no web search provider is configured"), "{e}");
    assert!(e.contains("will not fabricate search results"));
}

#[tokio::test]
async fn the_capability_report_separates_local_from_network_work() {
    let (st, _) = friday::test_state().await.unwrap();
    st.config.write().await.mode = "offline".into();
    let ctx = ToolCtx::root(st.clone(), None);
    let out = tools::execute(
        &ctx,
        "offline_capability_report",
        json!({"request":"research io_uring and summarize my notes"}),
    )
    .await;
    assert!(out.ok);
    let local: Vec<&str> = out.value["offline_capable_tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    let net: Vec<&str> = out.value["network_dependent_tools"]
        .as_array()
        .unwrap()
        .iter()
        .map(|v| v.as_str().unwrap())
        .collect();
    assert!(local.contains(&"search_knowledge") && local.contains(&"read_local_file"));
    assert!(net.contains(&"search_web") && net.contains(&"fetch_web_page"));
    assert!(
        !local.iter().any(|t| net.contains(t)),
        "a tool cannot be in both lists"
    );

    // With no local model, the report says so instead of promising an answer.
    assert_eq!(out.value["model"]["available"], json!(false));
    assert!(out.value["model"]["why_not"]
        .as_str()
        .unwrap()
        .contains("local model"));
}

#[tokio::test]
async fn reconciling_a_claim_reports_absence_of_evidence_as_such() {
    let st = library().await;
    let ctx = ToolCtx::root(st.clone(), None);

    let known = tools::execute(
        &ctx,
        "reconcile_claims",
        json!({"claim":"io_uring reduces syscall overhead"}),
    )
    .await;
    assert_eq!(known.value["confidence"], json!("local_evidence_available"));

    let unknown = tools::execute(
        &ctx,
        "reconcile_claims",
        json!({"claim":"gravity reverses on Tuesdays"}),
    )
    .await;
    assert_eq!(unknown.value["confidence"], json!("no_local_evidence"));
    assert!(unknown.value["instruction"]
        .as_str()
        .unwrap()
        .contains("rather than guessing"));
}

#[tokio::test]
async fn exported_knowledge_is_markdown_with_provenance() {
    let st = library().await;
    friday::memory::upsert(
        &st,
        &json!({"content":"prefers Rust","category":"profile","source":"user message","provenance":"user_stated"}),
        true,
    )
    .await
    .unwrap();
    let ctx = ToolCtx::root(st.clone(), None);
    let out = tools::execute(&ctx, "export_knowledge", json!({})).await;
    assert!(out.ok);
    let md = out.value["markdown"].as_str().unwrap();
    assert!(md.contains("## Memories") && md.contains("## Research reports"));
    assert!(
        md.contains("source: user message"),
        "an export must keep provenance"
    );
    assert!(md.contains("io_uring"));
}

#[tokio::test]
async fn a_knowledge_timeline_points_at_the_records_it_came_from() {
    let st = library().await;
    let ctx = ToolCtx::root(st.clone(), None);
    friday::tools::execute(
        &ctx,
        "create_decision_record",
        json!({"title":"Use epoll for v1","decision":"epoll","alternatives":["io_uring"],
               "evidence":["https://man7.org/linux/man-pages/man7/epoll.7.html"],
               "revisit_when":"when the minimum kernel is 6.1+"}),
    )
    .await;
    let out = tools::execute(&ctx, "build_personal_knowledge_timeline", json!({})).await;
    assert!(out.ok);
    let items = out.value["timeline"].as_array().unwrap();
    assert!(items.iter().any(|i| i["kind"] == "decision"));
    assert!(items.iter().any(|i| i["kind"] == "research"));
    assert!(items.iter().all(|i| i["record_id"].is_string()));
}
