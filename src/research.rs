//! Web research and the local knowledge library. Online-to-Offline handoff:
//! every report stores its question, sources, fetch timestamps and extracted
//! evidence so it stays usable with no network.
use crate::state::AppState;
use crate::util::{now, strip_html, truncate, uid, validate_public_url};
use anyhow::{bail, Result};
use serde_json::{json, Value};

#[derive(Debug, Clone)]
pub struct Source {
    pub url: String,
    pub title: String,
    pub snippet: String,
    pub published: Option<String>,
}

/// Tavily is used when `TAVILY_API_KEY` is set. Without it the tool fails
/// honestly instead of inventing results.
pub async fn search_web(
    state: &AppState,
    query: &str,
    limit: usize,
    domains: &[String],
) -> Result<Vec<Source>> {
    let cfg = state.cfg().await;
    if cfg.research.search_provider != "tavily" {
        bail!(
            "unsupported research.search_provider {:?}",
            cfg.research.search_provider
        );
    }
    let key = std::env::var("TAVILY_API_KEY")
        .ok()
        .filter(|k| !k.trim().is_empty())
        .ok_or_else(|| {
            anyhow::anyhow!(
            "no web search provider is configured. Set TAVILY_API_KEY in .env (see .env.example) \
             or answer from search_knowledge instead. FRIDAY will not fabricate search results."
        )
        })?;
    let mut body = json!({
        "api_key": key, "query": query, "max_results": limit.clamp(1, 20),
        "search_depth": "basic", "include_answer": false
    });
    if !domains.is_empty() {
        body["include_domains"] = json!(domains);
    }
    let resp = state
        .http
        .post("https://api.tavily.com/search")
        .json(&body)
        .send()
        .await?;
    let status = resp.status();
    let text = resp.text().await.unwrap_or_default();
    if !status.is_success() {
        bail!(
            "search provider returned {status}: {}",
            truncate(&crate::util::redact(&text), 300)
        );
    }
    let v: Value = serde_json::from_str(&text)?;
    let out = v["results"]
        .as_array()
        .map(|a| {
            a.iter()
                .filter_map(|r| {
                    let url = r["url"].as_str()?.to_string();
                    Some(Source {
                        url,
                        title: r["title"].as_str().unwrap_or("").to_string(),
                        snippet: r["content"].as_str().unwrap_or("").to_string(),
                        published: r["published_date"].as_str().map(str::to_string),
                    })
                })
                .collect::<Vec<_>>()
        })
        .unwrap_or_default();
    Ok(out)
}

pub async fn fetch_page(state: &AppState, raw_url: &str) -> Result<(String, String)> {
    let url = validate_public_url(raw_url)?;
    let cfg = state.cfg().await;
    let resp = state
        .http
        .get(url.clone())
        .header("accept", "text/html,text/plain;q=0.9")
        .send()
        .await?;
    // A cross-host redirect is followed by reqwest; re-validate the final URL.
    let final_url = resp.url().clone();
    validate_public_url(final_url.as_str())?;
    let status = resp.status();
    if !status.is_success() {
        bail!("{final_url} returned {status}");
    }
    let ct = resp
        .headers()
        .get(reqwest::header::CONTENT_TYPE)
        .and_then(|v| v.to_str().ok())
        .unwrap_or("")
        .to_string();
    if !(ct.contains("text/") || ct.contains("json") || ct.is_empty()) {
        bail!("{final_url} has unsupported content type {ct:?}");
    }
    let bytes = resp.bytes().await?;
    let text =
        String::from_utf8_lossy(&bytes[..bytes.len().min(cfg.research.max_page_bytes)]).to_string();
    let body = if ct.contains("html") || text.trim_start().starts_with('<') {
        strip_html(&text)
    } else {
        text
    };
    Ok((final_url.to_string(), body))
}

pub async fn save_report(
    state: &AppState,
    title: &str,
    question: &str,
    summary: &str,
    content: &str,
    sources: &[Source],
    tags: Value,
    freshness_days: i64,
) -> Result<String> {
    let id = uid();
    sqlx::query(
        "INSERT INTO research_documents (id,title,question,summary,content,tags,freshness_days,created_at)
         VALUES (?,?,?,?,?,?,?,?)",
    )
    .bind(&id).bind(title).bind(question).bind(summary).bind(content)
    .bind(tags.to_string()).bind(freshness_days).bind(now())
    .execute(&state.db).await?;
    sqlx::query("INSERT INTO research_fts (id,title,summary,content) VALUES (?,?,?,?)")
        .bind(&id)
        .bind(title)
        .bind(summary)
        .bind(content)
        .execute(&state.db)
        .await?;
    for src in sources {
        sqlx::query(
            "INSERT INTO research_sources (id,document_id,url,title,snippet,published,fetched_at) VALUES (?,?,?,?,?,?,?)")
            .bind(uid()).bind(&id).bind(&src.url).bind(&src.title)
            .bind(truncate(&src.snippet, 2000)).bind(&src.published).bind(now())
            .execute(&state.db).await?;
    }
    state
        .audit("system", "research", "save_report", title, None)
        .await;
    Ok(id)
}

pub async fn search(state: &AppState, query: &str, limit: i64) -> Result<Vec<Value>> {
    let fts = crate::memory::sanitize_fts(query);
    let mut rows: Vec<(String, String, String, String, i64)> = if fts.is_empty() {
        vec![]
    } else {
        sqlx::query_as(
            "SELECT d.id,d.title,d.summary,d.created_at,d.freshness_days
             FROM research_fts f JOIN research_documents d ON d.id=f.id
             WHERE research_fts MATCH ? ORDER BY rank LIMIT ?",
        )
        .bind(&fts)
        .bind(limit)
        .fetch_all(&state.db)
        .await
        .unwrap_or_default()
    };
    if rows.is_empty() {
        rows = sqlx::query_as(
            "SELECT id,title,summary,created_at,freshness_days FROM research_documents
             WHERE title LIKE ? OR summary LIKE ? OR content LIKE ? ORDER BY created_at DESC LIMIT ?")
            .bind(format!("%{query}%")).bind(format!("%{query}%")).bind(format!("%{query}%"))
            .bind(limit).fetch_all(&state.db).await?;
    }
    let mut out = Vec::new();
    for (id, title, summary, created_at, fresh_days) in rows {
        let urls: Vec<String> =
            sqlx::query_scalar("SELECT url FROM research_sources WHERE document_id=?")
                .bind(&id)
                .fetch_all(&state.db)
                .await
                .unwrap_or_default();
        let age_days = chrono::DateTime::parse_from_rfc3339(&created_at)
            .map(|t| (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_days())
            .unwrap_or(0);
        out.push(json!({
            "id": id, "title": title, "summary": summary, "created_at": created_at,
            "age_days": age_days, "stale": age_days > fresh_days, "sources": urls
        }));
    }
    Ok(out)
}

pub async fn get_report(state: &AppState, id: &str) -> Result<Value> {
    let row: Option<(String, String, String, String, String, String)> = sqlx::query_as(
        "SELECT id,title,question,summary,content,created_at FROM research_documents WHERE id=?",
    )
    .bind(id)
    .fetch_optional(&state.db)
    .await?;
    let (id, title, question, summary, content, created_at) =
        row.ok_or_else(|| anyhow::anyhow!("no research report {id}"))?;
    let srcs: Vec<(String, String, Option<String>, String)> = sqlx::query_as(
        "SELECT url,title,published,fetched_at FROM research_sources WHERE document_id=?",
    )
    .bind(&id)
    .fetch_all(&state.db)
    .await?;
    Ok(json!({
        "id": id, "title": title, "question": question, "summary": summary,
        "content": content, "created_at": created_at,
        "sources": srcs.into_iter().map(|(url,t,published,fetched_at)|
            json!({"url":url,"title":t,"published":published,"fetched_at":fetched_at})).collect::<Vec<_>>()
    }))
}

/// U2: reports past their per-topic freshness policy.
pub async fn freshness_check(state: &AppState) -> Result<Vec<Value>> {
    let rows: Vec<(String, String, String, i64)> =
        sqlx::query_as("SELECT id,title,created_at,freshness_days FROM research_documents")
            .fetch_all(&state.db)
            .await?;
    Ok(rows
        .into_iter()
        .filter_map(|(id, title, created_at, fresh)| {
            let age = chrono::DateTime::parse_from_rfc3339(&created_at)
                .map(|t| (chrono::Utc::now() - t.with_timezone(&chrono::Utc)).num_days())
                .unwrap_or(0);
            (age > fresh).then(|| {
                json!({"id":id,"title":title,"age_days":age,"freshness_days":fresh,
                "suggestion":"re-run research_topic to refresh this report"})
            })
        })
        .collect())
}
