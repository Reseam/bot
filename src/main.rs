mod agent;
mod config;
mod llm;
#[cfg(test)]
mod test_support;
mod tools;

use anyhow::{Context, Result};
use tracing::info;
use tracing_subscriber::EnvFilter;

use crate::config::Config;

#[tokio::main]
async fn main() -> Result<()> {
    if let Err(error) = dotenvy::dotenv()
        && !error.not_found()
    {
        return Err(error).context("failed to load .env");
    }

    let filter =
        EnvFilter::try_from_env("RESEAM_BOT_LOG").unwrap_or_else(|_| EnvFilter::new("info"));
    tracing_subscriber::fmt().with_env_filter(filter).init();

    let config = Config::load()?;
    info!(data_dir = %config.data_dir.display(), "configuration loaded");
    Ok(())
}
