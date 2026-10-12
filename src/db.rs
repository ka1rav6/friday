use anyhow::Result;
use sqlx::sqlite::{SqliteConnectOptions, SqlitePoolOptions};
use sqlx::SqlitePool;
use std::str::FromStr;

pub async fn connect(url: &str) -> Result<SqlitePool> {
    if let Some(path) = url
        .strip_prefix("sqlite://")
        .and_then(|p| p.split('?').next())
    {
        if let Some(parent) = std::path::Path::new(path).parent() {
            if !parent.as_os_str().is_empty() {
                std::fs::create_dir_all(parent)?;
            }
        }
    }
    let opts = SqliteConnectOptions::from_str(url)?
        .create_if_missing(true)
        .foreign_keys(true)
        .busy_timeout(std::time::Duration::from_secs(10));
    let pool = SqlitePoolOptions::new()
        .max_connections(8)
        .connect_with(opts)
        .await?;
    sqlx::query("PRAGMA journal_mode=WAL")
        .execute(&pool)
        .await
        .ok();
    migrate(&pool).await?;
    Ok(pool)
}

/// Applies the bundled SQL migrations. Statements are idempotent
/// (`CREATE ... IF NOT EXISTS`) and tracked in `_migrations`.
pub async fn migrate(pool: &SqlitePool) -> Result<()> {
    sqlx::query(
        "CREATE TABLE IF NOT EXISTS _migrations (name TEXT PRIMARY KEY, applied_at TEXT NOT NULL)",
    )
    .execute(pool)
    .await?;
    for (name, sql) in MIGRATIONS {
        let done: Option<(String,)> = sqlx::query_as("SELECT name FROM _migrations WHERE name = ?")
            .bind(name)
            .fetch_optional(pool)
            .await?;
        if done.is_some() {
            continue;
        }
        let mut tx = pool.begin().await?;
        for stmt in split_statements(sql) {
            sqlx::query(&stmt).execute(&mut *tx).await.map_err(|e| {
                anyhow::anyhow!(
                    "migration {name} failed on statement `{}`: {e}",
                    &stmt[..stmt.len().min(80)]
                )
            })?;
        }
        sqlx::query("INSERT INTO _migrations (name, applied_at) VALUES (?, ?)")
            .bind(name)
            .bind(crate::util::now())
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        tracing::info!(migration = name, "applied");
    }
    Ok(())
}

const MIGRATIONS: &[(&str, &str)] = &[("0001_init", include_str!("../migrations/0001_init.sql"))];

fn split_statements(sql: &str) -> Vec<String> {
    sql.split(';')
        .map(|s| s.trim().to_string())
        .filter(|s| !s.is_empty() && !s.lines().all(|l| l.trim_start().starts_with("--")))
        .collect()
}
