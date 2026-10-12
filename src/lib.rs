//! FRIDAY - Personal AI Operating System.
//!
//! The crate is a library so that integration tests can drive the real stack
//! (database, tool registry, agent loop) rather than a parallel test harness.
//
// sqlx's runtime query API returns rows as tuples, so a `SELECT` of several
// columns is unavoidably a wide tuple type. Naming an alias for each one would
// add indirection without making the queries clearer.
#![allow(clippy::type_complexity)]
pub mod agents;
pub mod api;
pub mod briefing;
pub mod config;
pub mod db;
pub mod evidence;
pub mod gmail;
pub mod media;
pub mod memory;
pub mod models;
pub mod research;
pub mod scheduler;
pub mod skills;
pub mod state;
pub mod tools;
pub mod util;

/// Loads `.env` so live tests and the CLI see the same credentials the server
/// does. Safe to call more than once.
pub fn load_env() {
    dotenvy::dotenv().ok();
}

/// Builds an application state wired to the deterministic mock provider, for
/// tests and offline inspection. It never contacts a hosted model.
///
/// Returns the shared `MockProvider` so a test can script the exact sequence of
/// model turns the agent loop will see.
pub async fn test_state_at(
    db_url: &str,
) -> anyhow::Result<(state::AppState, std::sync::Arc<models::MockProvider>)> {
    use std::sync::Arc;
    let db = db::connect(db_url).await?;
    let mut cfg = config::Config::default();
    cfg.model.provider = "mock".into();
    cfg.database_url = db_url.to_string();
    let loaded = config::Loaded {
        config: cfg,
        sources: config::Sources {
            user_config: None,
            project_config: None,
            env_overrides: vec![],
        },
    };
    let st = state::AppState::new(db, loaded);
    let mock = Arc::new(models::MockProvider::new());
    *st.router.write().await = Arc::new(models::Router {
        mock: mock.clone() as Arc<dyn models::ModelProvider>,
        ..Default::default()
    });
    // `Router.mock` is type-erased; hand the concrete handle back for scripting.
    Ok((st, mock))
}

/// In-memory variant for tests that do not need the database to survive a
/// simulated restart.
pub async fn test_state() -> anyhow::Result<(state::AppState, std::sync::Arc<models::MockProvider>)>
{
    test_state_at("sqlite://:memory:").await
}
