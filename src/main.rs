mod access;
mod agent;
mod attachments;
mod chat;
mod commands;
mod config;
mod conversations;
mod db;
mod forge;
mod llm;
mod mcp;
mod moderation;
#[cfg(test)]
mod test_support;
mod text;
mod tools;

use std::sync::Arc;

use anyhow::{Context, Result};
use poise::serenity_prelude as serenity;
use tracing::{error, info};
use tracing_subscriber::EnvFilter;

use crate::chat::Runs;
use crate::config::Config;
use crate::llm::Llm;
use crate::tools::repo::RepoLocks;

pub type Error = anyhow::Error;
pub type Data = Arc<App>;

pub struct App {
    pub config: Config,
    pub db: sqlx::SqlitePool,
    pub llm: Llm,
    pub http: reqwest::Client,
    pub runs: Runs,
    pub forges: std::collections::BTreeMap<String, forge::Forge>,
    pub repo_locks: RepoLocks,
    pub mcp: mcp::Mcp,
}

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
    let db = db::open(&config.data_dir).await?;
    conversations::spawn_pruning(db.clone(), config.agent.conversation_retention_days);
    let token = config.discord.token.clone();
    let guild_id = config.discord.guild_id;
    let llm = Llm::new(config.llm.clone())?;
    let mcp = mcp::Mcp::connect(&config.mcp).await;
    let forges = config
        .forges
        .iter()
        .map(|(name, config)| Ok((name.clone(), forge::Forge::new(config)?)))
        .collect::<Result<std::collections::BTreeMap<_, _>>>()?;
    let http = reqwest::Client::builder()
        .connect_timeout(std::time::Duration::from_secs(30))
        .timeout(std::time::Duration::from_secs(120))
        .build()
        .context("failed to build attachment HTTP client")?;
    let app = Arc::new(App {
        config,
        db,
        llm,
        http,
        runs: Runs::default(),
        forges,
        repo_locks: RepoLocks::default(),
        mcp,
    });
    info!(data_dir = %app.config.data_dir.display(), "configuration loaded");

    let setup_app = app.clone();
    let framework = poise::Framework::builder()
        .options(poise::FrameworkOptions {
            commands: commands::all(),
            event_handler: |framework, event| Box::pin(chat::event_handler(framework, event)),
            on_error: |error| Box::pin(on_error(error)),
            allowed_mentions: Some(serenity::CreateAllowedMentions::new()),
            owners: app.config.access.owner_ids.iter().copied().collect(),
            initialize_owners: false,
            prefix_options: poise::PrefixFrameworkOptions {
                mention_as_prefix: false,
                ..Default::default()
            },
            ..Default::default()
        })
        .setup(move |ctx, ready, framework| {
            let app = setup_app.clone();
            Box::pin(async move {
                info!(user = %ready.user.name, user_id = %ready.user.id, "connected to Discord");
                moderation::spawn_expired_bans(ctx.clone(), app.db.clone());
                poise::builtins::register_in_guild(ctx, &framework.options().commands, guild_id)
                    .await
                    .context("failed to register guild commands")?;
                info!(%guild_id, "registered guild commands");
                Ok(app)
            })
        })
        .build();
    let intents = serenity::GatewayIntents::non_privileged()
        | serenity::GatewayIntents::MESSAGE_CONTENT
        | serenity::GatewayIntents::GUILD_MEMBERS;
    let mut client = serenity::ClientBuilder::new(token, intents)
        .framework(framework)
        .await
        .context("failed to build Discord client")?;
    let shard_manager = client.shard_manager.clone();

    let discord_result = tokio::select! {
        result = client.start() => result.context("Discord client stopped with an error"),
        result = shutdown_signal() => {
            match result {
                Ok(()) => {
                    info!("shutdown signal received");
                    app.runs.cancel_all();
                    shard_manager.shutdown_all().await;
                    info!("Discord shards shut down");
                    Ok(())
                }
                Err(error) => Err(error),
            }
        }
    };
    app.mcp.shutdown().await;
    info!("MCP clients shut down");
    discord_result
}

async fn on_error(error: poise::FrameworkError<'_, Data, Error>) {
    match error {
        poise::FrameworkError::CommandCheckFailed { ctx, .. } => {
            if let Err(error) = ctx
                .send(
                    poise::CreateReply::new()
                        .content("Only the Reseam team can use this.")
                        .ephemeral(true),
                )
                .await
            {
                error!(?error, "failed to send access denial");
            }
        }
        poise::FrameworkError::GuildOnly { .. } => {}
        other => {
            error!(error = %other, "Discord framework error");
            if let Err(error) = poise::builtins::on_error(other).await {
                error!(?error, "failed to report Discord framework error");
            }
        }
    }
}

#[cfg(unix)]
async fn shutdown_signal() -> Result<()> {
    use tokio::signal::unix::{SignalKind, signal};

    let mut interrupt = signal(SignalKind::interrupt()).context("failed to listen for SIGINT")?;
    let mut terminate = signal(SignalKind::terminate()).context("failed to listen for SIGTERM")?;
    tokio::select! {
        _ = interrupt.recv() => {}
        _ = terminate.recv() => {}
    }
    Ok(())
}

#[cfg(not(unix))]
async fn shutdown_signal() -> Result<()> {
    tokio::signal::ctrl_c()
        .await
        .context("failed to listen for Ctrl+C")
}
