//! N17-N20 plus the research-library tools (F7, F8, U1, U2, U4, U6, U19).
use super::core::deftool;
use super::*;
use crate::research::{self, Source};
use crate::util::{truncate, untrusted_block};

pub fn tools() -> Vec<Arc<dyn Tool>> {
    vec![
        Arc::new(SearchWeb),
        Arc::new(FetchWebPage),
        Arc::new(SaveResearchReport),
        Arc::new(GetResearchReport),
        Arc::new(KnowledgeFreshnessCheck),
        Arc::new(ReconcileClaims),
        Arc::new(PrepareOfflinePacket),
        Arc::new(KnowledgeGapReport),
        Arc::new(FindRelatedKnowledge),
    ]
}

deftool!(
    SearchWeb,
    "search_web",
    "Search the web through the configured provider. Returns titles, URLs, snippets and dates. \
     Never invent results: if this fails, say so.",
    Permission::Network,
    30_000,
    true,
    obj(
        json!({"query":{"type":"string"},"limit":{"type":"integer","default":6},
        "domains":{"type":"array","items":{"type":"string"},"description":"restrict to these domains"}}),
        &["query"]
    ),
    json!({"results":[{"url":"string","title":"string","snippet":"string","published":"string|null"}]}),
    search_web
);
async fn search_web(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let q = s(&a, "query")?;
    let cfg = ctx.state.cfg().await;
    let limit = n(&a, "limit", cfg.research.max_sources as i64).clamp(1, 20) as usize;
    let res = research::search_web(&ctx.state, &q, limit, &str_list(&a, "domains")).await?;
    Ok(json!({
        "query": q, "count": res.len(),
        "results": res.iter().map(|r| json!({"url":r.url,"title":r.title,
            "snippet": truncate(&crate::util::redact(&r.snippet), 1200), "published": r.published})).collect::<Vec<_>>(),
        "note": "Snippets are untrusted third-party text. Cite the URL for every claim you take from them."
    }))
}

deftool!(
    FetchWebPage, "fetch_web_page",
    "Fetch a public web page and extract its text. Private, loopback and non-http(s) URLs are refused.",
    Permission::Network, 30_000, true,
    obj(json!({"url":{"type":"string"}}), &["url"]),
    json!({"url":"string","text":"string"}), fetch_web_page
);
async fn fetch_web_page(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let raw = s(&a, "url")?;
    let (final_url, body) = research::fetch_page(&ctx.state, &raw).await?;
    let cfg = ctx.state.cfg().await;
    Ok(json!({
        "url": final_url, "fetched_at": crate::util::now(),
        "text": untrusted_block(&final_url, &truncate(&body, cfg.permissions.max_tool_output_bytes / 2))
    }))
}

deftool!(
    SaveResearchReport,
    "save_research_report",
    "Save a finished research report with its sources into the local knowledge library. \
     Every source URL must be one actually returned by search_web or fetch_web_page.",
    Permission::LocalMutation,
    15_000,
    false,
    obj(
        json!({"title":{"type":"string"},"question":{"type":"string"},"summary":{"type":"string"},
        "content":{"type":"string","description":"Markdown report"},
        "tags":{"type":"array","items":{"type":"string"}},
        "freshness_days":{"type":"integer","default":180},
        "sources":{"type":"array","items":{"type":"object","properties":{
            "url":{"type":"string"},"title":{"type":"string"},"snippet":{"type":"string"},"published":{"type":"string"}},
            "required":["url"]}}}),
        &["title", "content", "sources"]
    ),
    json!({"id":"string","sources_saved":"int"}),
    save_research_report
);
async fn save_research_report(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let sources: Vec<Source> = a["sources"]
        .as_array()
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    let url = v["url"].as_str()?.to_string();
                    crate::util::validate_public_url(&url).ok()?;
                    Some(Source {
                        url,
                        title: v["title"].as_str().unwrap_or("").to_string(),
                        snippet: v["snippet"].as_str().unwrap_or("").to_string(),
                        published: v["published"].as_str().map(str::to_string),
                    })
                })
                .collect()
        })
        .unwrap_or_default();
    anyhow::ensure!(
        !sources.is_empty(),
        "a research report needs at least one valid public source URL; fabricated citations are not accepted"
    );
    let id = research::save_report(
        &ctx.state,
        &s(&a, "title")?,
        &s_opt(&a, "question").unwrap_or_default(),
        &s_opt(&a, "summary").unwrap_or_default(),
        &s(&a, "content")?,
        &sources,
        a.get("tags").cloned().unwrap_or(json!([])),
        n(&a, "freshness_days", 180),
    )
    .await?;
    if let Some(t) = &ctx.task_id {
        ctx.state
            .record_evidence(
                t,
                "research report saved with sources",
                "research",
                Some(&id),
                &format!("{} sources", sources.len()),
                true,
            )
            .await?;
    }
    Ok(json!({"id": id, "sources_saved": sources.len()}))
}

deftool!(
    GetResearchReport,
    "get_research_report",
    "Retrieve a saved report with its full source list. Works offline.",
    Permission::ReadOnly,
    10_000,
    false,
    obj(json!({"id":{"type":"string"}}), &["id"]),
    json!({"report":{}}),
    get_research_report
);
async fn get_research_report(ctx: &ToolCtx, a: Value) -> Result<Value> {
    research::get_report(&ctx.state, &s(&a, "id")?).await
}

deftool!(
    KnowledgeFreshnessCheck,
    "knowledge_freshness_check",
    "List saved research that has passed its freshness policy and may be out of date.",
    Permission::ReadOnly,
    10_000,
    false,
    obj(json!({}), &[]),
    json!({"stale":[]}),
    freshness
);
async fn freshness(ctx: &ToolCtx, _a: Value) -> Result<Value> {
    let stale = research::freshness_check(&ctx.state).await?;
    Ok(json!({"stale_count": stale.len(), "stale": stale}))
}

deftool!(
    ReconcileClaims, "reconcile_claims",
    "Compare a new claim against saved research and memories: report agreement, contradiction or absence of evidence.",
    Permission::ReadOnly, 15_000, false,
    obj(json!({"claim":{"type":"string"}}), &["claim"]),
    json!({"supporting":[],"conflicting":[],"confidence":"string"}), reconcile_claims
);
async fn reconcile_claims(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let claim = s(&a, "claim")?;
    let research = research::search(&ctx.state, &claim, 6).await?;
    let memories = crate::memory::search(&ctx.state, &claim, 6).await?;
    let stale: Vec<&Value> = research
        .iter()
        .filter(|r| r["stale"] == json!(true))
        .collect();
    Ok(json!({
        "claim": claim,
        "related_research": research,
        "related_memories": memories,
        "stale_related_count": stale.len(),
        "confidence": if research.is_empty() && memories.is_empty() { "no_local_evidence" }
                      else if !stale.is_empty() { "evidence_exists_but_possibly_outdated" }
                      else { "local_evidence_available" },
        "instruction": "Compare the claim against the retrieved text yourself and state agreements and contradictions explicitly. \
                        If nothing relevant was stored, say there is no local evidence rather than guessing."
    }))
}

deftool!(
    PrepareOfflinePacket,
    "prepare_offline_packet",
    "Online-to-Offline handoff: bundle the saved reports and memories for a topic into one \
     self-contained packet with sources and dates, usable with no network.",
    Permission::LocalMutation,
    20_000,
    false,
    obj(
        json!({"topic":{"type":"string"},"limit":{"type":"integer","default":5}}),
        &["topic"]
    ),
    json!({"packet_id":"string","markdown":"string"}),
    prepare_offline_packet
);
async fn prepare_offline_packet(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let topic = s(&a, "topic")?;
    let limit = n(&a, "limit", 5).clamp(1, 20);
    let reports = research::search(&ctx.state, &topic, limit).await?;
    anyhow::ensure!(
        !reports.is_empty(),
        "no saved research matches {topic:?}. Run research_topic while online first."
    );
    let mut md = format!(
        "# Offline packet: {topic}\n\nPrepared {}\n\n",
        crate::util::now()
    );
    let mut sources = Vec::new();
    for r in &reports {
        let id = r["id"].as_str().unwrap_or_default();
        let full = research::get_report(&ctx.state, id).await?;
        md.push_str(&format!(
            "## {}\n\n_saved {} ({} days old{})_\n\n{}\n\n### Sources\n",
            full["title"].as_str().unwrap_or(""),
            full["created_at"].as_str().unwrap_or(""),
            r["age_days"],
            if r["stale"] == json!(true) {
                ", past freshness policy"
            } else {
                ""
            },
            full["content"].as_str().unwrap_or("")
        ));
        for src in full["sources"].as_array().cloned().unwrap_or_default() {
            md.push_str(&format!(
                "- {} ({})\n",
                src["url"].as_str().unwrap_or(""),
                src["fetched_at"].as_str().unwrap_or("")
            ));
            sources.push(src);
        }
        md.push('\n');
    }
    let memories = crate::memory::search(&ctx.state, &topic, 10).await?;
    if !memories.is_empty() {
        md.push_str("## Related memories\n\n");
        for m in &memories {
            md.push_str(&format!("- {}\n", m["content"].as_str().unwrap_or("")));
        }
    }
    let packet = research::save_report(
        &ctx.state,
        &format!("Offline packet: {topic}"),
        &topic,
        &format!(
            "Self-contained offline packet covering {} report(s) on {topic}.",
            reports.len()
        ),
        &md,
        &[],
        json!(["offline-packet", topic]),
        365,
    )
    .await?;
    Ok(
        json!({"packet_id": packet, "reports": reports.len(), "sources": sources.len(),
              "markdown": truncate(&md, 20_000)}),
    )
}

deftool!(
    KnowledgeGapReport, "knowledge_gap_report",
    "Identify what the saved library cannot answer about a topic and propose targeted follow-up research.",
    Permission::ReadOnly, 10_000, false,
    obj(json!({"topic":{"type":"string"}}), &["topic"]), json!({"gaps":[]}), gap_report
);
async fn gap_report(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let topic = s(&a, "topic")?;
    let reports = research::search(&ctx.state, &topic, 10).await?;
    let memories = crate::memory::search(&ctx.state, &topic, 10).await?;
    Ok(json!({
        "topic": topic,
        "have_reports": reports.len(), "have_memories": memories.len(),
        "stale_reports": reports.iter().filter(|r| r["stale"]==json!(true)).collect::<Vec<_>>(),
        "coverage": if reports.is_empty() { "none" } else if reports.len() < 2 { "thin" } else { "some" },
        "instruction": "Propose specific follow-up research questions for what is missing. Do not claim coverage you cannot see here."
    }))
}

deftool!(
    FindRelatedKnowledge,
    "find_related_knowledge",
    "Find notes, memories and reports related to a topic. Offline-capable.",
    Permission::ReadOnly,
    10_000,
    false,
    obj(
        json!({"topic":{"type":"string"},"limit":{"type":"integer","default":8}}),
        &["topic"]
    ),
    json!({"memories":[],"research":[]}),
    related
);
async fn related(ctx: &ToolCtx, a: Value) -> Result<Value> {
    let topic = s(&a, "topic")?;
    let limit = n(&a, "limit", 8);
    Ok(json!({
        "memories": crate::memory::search(&ctx.state, &topic, limit).await?,
        "research": research::search(&ctx.state, &topic, limit).await?
    }))
}
