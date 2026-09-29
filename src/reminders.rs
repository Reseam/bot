use std::time::Duration;

use anyhow::{Context, Result, bail};
use poise::serenity_prelude as serenity;
use sqlx::{FromRow, SqlitePool};
use tracing::{error, warn};

use crate::db::{discord_id, stored_discord_id};
use crate::text::parse_duration;

pub const MAX_MESSAGE_CHARS: usize = 1_000;
const MAX_PER_USER: i64 = 25;
const MAX_DELAY: Duration = Duration::from_secs(365 * 24 * 60 * 60);
const POLL_INTERVAL: Duration = Duration::from_secs(10);

#[derive(FromRow)]
pub struct Reminder {
    pub id: i64,
    channel_id: i64,
    user_id: i64,
    pub message: String,
    created_at: i64,
    pub due_at: i64,
}

impl Reminder {
    pub fn channel_id(&self) -> Result<serenity::ChannelId> {
        Ok(serenity::ChannelId::new(stored_discord_id(
            self.channel_id,
        )?))
    }
}

pub struct NewReminder<'a> {
    pub guild_id: serenity::GuildId,
    pub channel_id: serenity::ChannelId,
    pub user_id: serenity::UserId,
    pub message: &'a str,
}

pub async fn schedule(db: &SqlitePool, reminder: &NewReminder<'_>, delay: &str) -> Result<i64> {
    let delay = parse_duration(delay)?;
    if delay > MAX_DELAY {
        bail!("reminders can be at most 365 days away");
    }
    if reminder.message.trim().is_empty() {
        bail!("the reminder message is empty");
    }
    if reminder.message.chars().count() > MAX_MESSAGE_CHARS {
        bail!("the reminder message is longer than {MAX_MESSAGE_CHARS} characters");
    }
    let created_at = serenity::Timestamp::now().unix_timestamp();
    let due_at = created_at + delay.as_secs() as i64;
    if !insert(db, reminder, created_at, due_at).await? {
        bail!("you already have {MAX_PER_USER} reminders, cancel one first");
    }
    Ok(due_at)
}

async fn insert(
    db: &SqlitePool,
    reminder: &NewReminder<'_>,
    created_at: i64,
    due_at: i64,
) -> Result<bool> {
    let guild_id = discord_id(reminder.guild_id.get())?;
    let user_id = discord_id(reminder.user_id.get())?;
    let result = sqlx::query(
        "INSERT INTO reminders (guild_id, channel_id, user_id, message, created_at, due_at) \
         SELECT ?, ?, ?, ?, ?, ? \
         WHERE (SELECT COUNT(*) FROM reminders WHERE guild_id = ? AND user_id = ?) < ?",
    )
    .bind(guild_id)
    .bind(discord_id(reminder.channel_id.get())?)
    .bind(user_id)
    .bind(reminder.message.trim())
    .bind(created_at)
    .bind(due_at)
    .bind(guild_id)
    .bind(user_id)
    .bind(MAX_PER_USER)
    .execute(db)
    .await
    .context("failed to save reminder")?;
    Ok(result.rows_affected() == 1)
}

pub async fn list(
    db: &SqlitePool,
    guild_id: serenity::GuildId,
    user_id: serenity::UserId,
) -> Result<Vec<Reminder>> {
    sqlx::query_as(
        "SELECT id, channel_id, user_id, message, created_at, due_at FROM reminders \
         WHERE guild_id = ? AND user_id = ? ORDER BY due_at",
    )
    .bind(discord_id(guild_id.get())?)
    .bind(discord_id(user_id.get())?)
    .fetch_all(db)
    .await
    .context("failed to list reminders")
}

pub async fn cancel(
    db: &SqlitePool,
    guild_id: serenity::GuildId,
    user_id: serenity::UserId,
    id: i64,
) -> Result<bool> {
    let result = sqlx::query("DELETE FROM reminders WHERE id = ? AND guild_id = ? AND user_id = ?")
        .bind(id)
        .bind(discord_id(guild_id.get())?)
        .bind(discord_id(user_id.get())?)
        .execute(db)
        .await
        .context("failed to cancel reminder")?;
    Ok(result.rows_affected() == 1)
}

async fn due(db: &SqlitePool, now: i64) -> Result<Vec<Reminder>> {
    sqlx::query_as(
        "SELECT id, channel_id, user_id, message, created_at, due_at FROM reminders \
         WHERE due_at <= ? ORDER BY due_at",
    )
    .bind(now)
    .fetch_all(db)
    .await
    .context("failed to find due reminders")
}

async fn remove(db: &SqlitePool, id: i64) -> Result<()> {
    sqlx::query("DELETE FROM reminders WHERE id = ?")
        .bind(id)
        .execute(db)
        .await
        .context("failed to remove delivered reminder")?;
    Ok(())
}

pub fn spawn(discord: serenity::Context, db: SqlitePool) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(POLL_INTERVAL);
        loop {
            interval.tick().await;
            if let Err(error) = deliver_due(&discord, &db).await {
                error!(error = %format!("{error:#}"), "failed to deliver reminders");
            }
        }
    });
}

async fn deliver_due(discord: &serenity::Context, db: &SqlitePool) -> Result<()> {
    for reminder in due(db, serenity::Timestamp::now().unix_timestamp()).await? {
        let channel_id = reminder.channel_id()?;
        let user_id = serenity::UserId::new(stored_discord_id(reminder.user_id)?);
        let message = serenity::CreateMessage::new()
            .content(format!(
                "<@{user_id}> Reminder from <t:{}:R>: {}",
                reminder.created_at, reminder.message
            ))
            .allowed_mentions(serenity::CreateAllowedMentions::new().users([user_id]));
        match channel_id.send_message(discord, message).await {
            Ok(_) => {}
            Err(serenity::Error::Http(serenity::HttpError::UnsuccessfulRequest(response)))
                if response.status_code.is_client_error() =>
            {
                warn!(
                    id = reminder.id,
                    %channel_id,
                    %user_id,
                    status = %response.status_code,
                    error = %response.error.message,
                    "dropping undeliverable reminder"
                );
            }
            Err(error) => {
                warn!(id = reminder.id, %channel_id, ?error, "failed to deliver reminder, will retry");
                continue;
            }
        }
        remove(db, reminder.id).await?;
    }
    Ok(())
}

#[cfg(test)]
mod tests;
