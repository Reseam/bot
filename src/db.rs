use std::path::Path;
use std::time::Duration;

use anyhow::{Context, Result};
use sqlx::SqlitePool;
use sqlx::sqlite::{SqliteConnectOptions, SqliteJournalMode, SqlitePoolOptions, SqliteSynchronous};

pub async fn open(data_dir: &Path) -> Result<SqlitePool> {
    tokio::fs::create_dir_all(data_dir)
        .await
        .with_context(|| format!("failed to create data directory {}", data_dir.display()))?;
    let options = SqliteConnectOptions::new()
        .filename(data_dir.join("bot.db"))
        .create_if_missing(true)
        .journal_mode(SqliteJournalMode::Wal)
        .synchronous(SqliteSynchronous::Normal)
        .busy_timeout(Duration::from_secs(5))
        .foreign_keys(true);
    let pool = SqlitePoolOptions::new()
        .connect_with(options)
        .await
        .context("failed to open SQLite database")?;
    sqlx::migrate!()
        .run(&pool)
        .await
        .context("failed to run database migrations")?;
    Ok(pool)
}
