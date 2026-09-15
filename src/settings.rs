use anyhow::{Context, Result};
use poise::serenity_prelude as serenity;
use sqlx::SqlitePool;

use crate::db::{discord_id, stored_discord_id};

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
