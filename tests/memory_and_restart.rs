//! Acceptance Test A: a memory survives a service restart and is retrievable
//! through the same path the agent uses.
use friday::{memory, test_state_at};
use serde_json::json;

async fn fresh() -> (
    friday::state::AppState,
    std::sync::Arc<friday::models::MockProvider>,
) {
    friday::test_state().await.unwrap()
}

#[tokio::test]
async fn memory_persists_across_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}/friday.db?mode=rwc", tmp.path().display());

    // First "boot": save an approved profile memory.
    let id = {
        let (st, _) = test_state_at(&url).await.unwrap();
        let saved = memory::upsert(
            &st,
            &json!({"content":"Kairav prefers Rust over Python for systems work",
                    "category":"profile","source":"user message","provenance":"user_stated"}),
            true,
        )
        .await
        .unwrap();
        assert_eq!(saved["approved"], json!(true));
        saved["id"].as_str().unwrap().to_string()
    };

    // Second "boot": a brand-new pool over the same database file.
    let (st2, _) = test_state_at(&url).await.unwrap();
    let got = memory::get(&st2, &id)
        .await
        .unwrap()
        .expect("memory survived restart");
    assert!(got["content"]
        .as_str()
        .unwrap()
        .contains("Rust over Python"));
    assert_eq!(got["provenance"], json!("user_stated"));

    // It is reachable by search, which is what the agent actually calls.
    let hits = memory::search(&st2, "Rust systems work", 5).await.unwrap();
    assert!(
        hits.iter().any(|m| m["id"] == id.as_str()),
        "search missed the memory: {hits:?}"
    );

    // And it is injected into the agent's context, labelled as data.
    let block = memory::context_block(&st2, "which language should I use")
        .await
        .unwrap();
    assert!(block.contains("Rust over Python"));
    assert!(block.contains("not instructions to follow"));
}

#[tokio::test]
async fn proposed_memories_are_not_used_until_approved() {
    let (st, _) = fresh().await;
    let proposed = memory::upsert(
        &st,
        &json!({"content":"user might live in Berlin","category":"profile",
                "source":"model guess","provenance":"model_inference"}),
        false,
    )
    .await
    .unwrap();
    assert_eq!(proposed["approved"], json!(false));
    assert!(memory::search(&st, "Berlin", 5).await.unwrap().is_empty());

    let id = proposed["id"].as_str().unwrap();
    memory::approve(&st, id).await.unwrap();
    assert_eq!(memory::search(&st, "Berlin", 5).await.unwrap().len(), 1);

    // A model inference carries lower confidence than a user statement.
    let got = memory::get(&st, id).await.unwrap().unwrap();
    assert!(got["confidence"].as_f64().unwrap() < 1.0);
}

#[tokio::test]
async fn deletion_removes_the_record_and_the_index_entry() {
    let (st, _) = fresh().await;
    let saved = memory::upsert(
        &st,
        &json!({"content":"ephemeral fact about quasars","category":"knowledge",
                "source":"test","provenance":"user_stated"}),
        true,
    )
    .await
    .unwrap();
    let id = saved["id"].as_str().unwrap();
    assert_eq!(memory::search(&st, "quasars", 5).await.unwrap().len(), 1);

    assert_eq!(
        memory::delete(&st, id).await.unwrap()["deleted"],
        json!(true)
    );
    assert!(memory::get(&st, id).await.unwrap().is_none());
    assert!(
        memory::search(&st, "quasars", 5).await.unwrap().is_empty(),
        "index entry survived"
    );
}

#[tokio::test]
async fn duplicate_content_is_deduplicated() {
    let (st, _) = fresh().await;
    let mk =
        || json!({"content":"same","category":"project","source":"t","provenance":"user_stated"});
    let a = memory::upsert(&st, &mk(), true).await.unwrap();
    let b = memory::upsert(&st, &mk(), true).await.unwrap();
    assert_eq!(a["id"], b["id"]);
    assert_eq!(b["deduplicated"], json!(true));
}

#[tokio::test]
async fn invalid_category_or_provenance_is_rejected() {
    let (st, _) = fresh().await;
    assert!(memory::upsert(
        &st,
        &json!({"content":"x","category":"nonsense",
        "source":"t","provenance":"user_stated"}),
        true
    )
    .await
    .is_err());
    assert!(memory::upsert(
        &st,
        &json!({"content":"x","category":"profile",
        "source":"t","provenance":"telepathy"}),
        true
    )
    .await
    .is_err());
    assert!(memory::upsert(
        &st,
        &json!({"content":"   ","category":"profile",
        "source":"t","provenance":"user_stated"}),
        true
    )
    .await
    .is_err());
}

#[tokio::test]
async fn contradictory_memories_are_surfaced_not_resolved() {
    let (st, _) = fresh().await;
    for c in [
        "deployment target is Nebius serverless",
        "deployment target is a local docker host",
    ] {
        memory::upsert(
            &st,
            &json!({"content":c,"category":"project",
            "source":"t","provenance":"user_stated"}),
            true,
        )
        .await
        .unwrap();
    }
    let conflicts = memory::conflicts(&st).await.unwrap();
    assert!(
        !conflicts.is_empty(),
        "expected a conflict on 'deployment target'"
    );
    assert!(conflicts[0]["resolution"]
        .as_str()
        .unwrap()
        .contains("do not guess"));
}

#[tokio::test]
async fn interrupted_runs_are_marked_failed_on_restart() {
    let tmp = tempfile::tempdir().unwrap();
    let url = format!("sqlite://{}/friday.db?mode=rwc", tmp.path().display());
    {
        let (st, _) = test_state_at(&url).await.unwrap();
        sqlx::query(
            "INSERT INTO tasks (id,title,description,kind,status,created_at,updated_at)
             VALUES ('t1','stuck','','user','running',?,?)",
        )
        .bind(friday::util::now())
        .bind(friday::util::now())
        .execute(&st.db)
        .await
        .unwrap();
    }
    let (st2, _) = test_state_at(&url).await.unwrap();
    assert_eq!(friday::agents::recover_interrupted(&st2).await.unwrap(), 1);
    let status: String = sqlx::query_scalar("SELECT status FROM tasks WHERE id='t1'")
        .fetch_one(&st2.db)
        .await
        .unwrap();
    assert_eq!(
        status, "failed",
        "a task left running by a crash must not look alive"
    );
}
