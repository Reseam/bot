use anyhow::{Context, Result};
use poise::serenity_prelude as serenity;
use sqlx::SqlitePool;

use crate::access::Tier;
use crate::db::{discord_id, stored_discord_id};
use crate::sandbox::SandboxKind;

pub async fn mod_log_channel(
    db: &SqlitePool,
    guild_id: serenity::GuildId,
) -> Result<Option<serenity::ChannelId>> {
    sqlx::query_scalar::<_, Option<i64>>(
        "SELECT mod_log_channel_id FROM guild_settings WHERE guild_id = ?",
    )
    .bind(discord_id(guild_id.get())?)
    .fetch_optional(db)
    .await
    .context("failed to read moderation log channel")?
    .flatten()
    .map(|id| stored_discord_id(id).map(serenity::ChannelId::new))
    .transpose()
}

pub async fn set_mod_log_channel(
    db: &SqlitePool,
    guild_id: serenity::GuildId,
    channel_id: serenity::ChannelId,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO guild_settings (guild_id, mod_log_channel_id) VALUES (?, ?) \
         ON CONFLICT(guild_id) DO UPDATE SET mod_log_channel_id = excluded.mod_log_channel_id",
    )
    .bind(discord_id(guild_id.get())?)
    .bind(discord_id(channel_id.get())?)
    .execute(db)
    .await
    .context("failed to save moderation log channel")?;
    Ok(())
}

pub async fn personality(db: &SqlitePool, guild_id: serenity::GuildId) -> Result<Option<String>> {
    Ok(sqlx::query_scalar::<_, Option<String>>(
        "SELECT personality FROM guild_settings WHERE guild_id = ?",
    )
    .bind(discord_id(guild_id.get())?)
    .fetch_optional(db)
    .await
    .context("failed to read personality")?
    .flatten())
}

pub async fn set_personality(
    db: &SqlitePool,
    guild_id: serenity::GuildId,
    personality: Option<&str>,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO guild_settings (guild_id, personality) VALUES (?, ?) \
         ON CONFLICT(guild_id) DO UPDATE SET personality = excluded.personality",
    )
    .bind(discord_id(guild_id.get())?)
    .bind(personality)
    .execute(db)
    .await
    .context("failed to save personality")?;
    Ok(())
}

pub async fn model(
    db: &SqlitePool,
    guild_id: serenity::GuildId,
    tier: Tier,
) -> Result<Option<String>> {
    let query = match tier {
        Tier::Team => "SELECT team_model FROM guild_settings WHERE guild_id = ?",
        Tier::Member => "SELECT member_model FROM guild_settings WHERE guild_id = ?",
    };
    Ok(sqlx::query_scalar::<_, Option<String>>(query)
        .bind(discord_id(guild_id.get())?)
        .fetch_optional(db)
        .await
        .context("failed to read model")?
        .flatten())
}

pub async fn set_model(
    db: &SqlitePool,
    guild_id: serenity::GuildId,
    tier: Tier,
    model: &str,
) -> Result<()> {
    let query = match tier {
        Tier::Team => {
            "INSERT INTO guild_settings (guild_id, team_model) VALUES (?, ?) \
             ON CONFLICT(guild_id) DO UPDATE SET team_model = excluded.team_model"
        }
        Tier::Member => {
            "INSERT INTO guild_settings (guild_id, member_model) VALUES (?, ?) \
             ON CONFLICT(guild_id) DO UPDATE SET member_model = excluded.member_model"
        }
    };
    sqlx::query(query)
        .bind(discord_id(guild_id.get())?)
        .bind(model)
        .execute(db)
        .await
        .context("failed to save model")?;
    Ok(())
}

pub async fn sandbox(db: &SqlitePool, guild_id: serenity::GuildId) -> Result<Option<SandboxKind>> {
    Ok(sqlx::query_scalar::<_, Option<SandboxKind>>(
        "SELECT sandbox FROM guild_settings WHERE guild_id = ?",
    )
    .bind(discord_id(guild_id.get())?)
    .fetch_optional(db)
    .await
    .context("failed to read sandbox setting")?
    .flatten())
}

pub async fn set_sandbox(
    db: &SqlitePool,
    guild_id: serenity::GuildId,
    sandbox: SandboxKind,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO guild_settings (guild_id, sandbox) VALUES (?, ?) \
         ON CONFLICT(guild_id) DO UPDATE SET sandbox = excluded.sandbox",
    )
    .bind(discord_id(guild_id.get())?)
    .bind(sandbox)
    .execute(db)
    .await
    .context("failed to save sandbox setting")?;
    Ok(())
}
