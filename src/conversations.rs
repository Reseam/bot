use std::time::Duration;

use anyhow::{Context, Result};
use poise::serenity_prelude as serenity;
use sqlx::{FromRow, SqlitePool};
use tracing::{error, info};

use crate::llm::{ContentPart, Message, UserContent};

pub struct Conversation {
    pub id: i64,
    pub transcript: Vec<Message>,
}

pub struct Save<'a> {
    pub id: Option<i64>,
    pub guild_id: serenity::GuildId,
    pub channel_id: serenity::ChannelId,
    pub started_by: serenity::UserId,
    pub transcript: &'a [Message],
    pub message_ids: &'a [serenity::MessageId],
}

#[derive(FromRow)]
struct ConversationRow {
    id: i64,
    transcript: String,
}

pub async fn find_by_message(
    db: &SqlitePool,
    message_id: serenity::MessageId,
) -> Result<Option<Conversation>> {
    let message_id = discord_id(message_id.get())?;
    let row = sqlx::query_as::<_, ConversationRow>(
        "SELECT c.id, c.transcript FROM conversations c \
         JOIN conversation_messages m ON m.conversation_id = c.id \
         WHERE m.message_id = ?",
    )
    .bind(message_id)
    .fetch_optional(db)
    .await
    .context("failed to find conversation by Discord message")?;
    row.map(|row| {
        Ok(Conversation {
            id: row.id,
            transcript: serde_json::from_str(&row.transcript)
                .context("failed to deserialize conversation transcript")?,
        })
    })
    .transpose()
}

pub async fn last_message_id(
    db: &SqlitePool,
    conversation_id: i64,
) -> Result<Option<serenity::MessageId>> {
    let id = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT MAX(message_id) FROM conversation_messages WHERE conversation_id = ?",
    )
    .bind(conversation_id)
    .fetch_one(db)
    .await
    .context("failed to find the conversation's last Discord message")?;
    id.map(|id| {
        u64::try_from(id)
            .map(serenity::MessageId::new)
            .context("stored Discord message ID is negative")
    })
    .transpose()
}

pub async fn save(db: &SqlitePool, save: Save<'_>) -> Result<i64> {
    let now = serenity::Timestamp::now().unix_timestamp();
    let transcript = serde_json::to_string(&strip_images(save.transcript))
        .context("failed to serialize conversation transcript")?;
    let mut transaction = db
        .begin()
        .await
        .context("failed to begin conversation transaction")?;
    let id = match save.id {
        Some(id) => {
            sqlx::query("UPDATE conversations SET transcript = ?, updated_at = ? WHERE id = ?")
                .bind(&transcript)
                .bind(now)
                .bind(id)
                .execute(&mut *transaction)
                .await
                .context("failed to update conversation")?;
            id
        }
        None => sqlx::query(
            "INSERT INTO conversations \
             (guild_id, channel_id, started_by, transcript, created_at, updated_at) \
             VALUES (?, ?, ?, ?, ?, ?)",
        )
        .bind(discord_id(save.guild_id.get())?)
        .bind(discord_id(save.channel_id.get())?)
        .bind(discord_id(save.started_by.get())?)
        .bind(&transcript)
        .bind(now)
        .bind(now)
        .execute(&mut *transaction)
        .await
        .context("failed to insert conversation")?
        .last_insert_rowid(),
    };
    for message_id in save.message_ids {
        sqlx::query(
            "INSERT OR IGNORE INTO conversation_messages (message_id, conversation_id) \
             VALUES (?, ?)",
        )
        .bind(discord_id(message_id.get())?)
        .bind(id)
        .execute(&mut *transaction)
        .await
        .context("failed to map Discord message to conversation")?;
    }
    transaction
        .commit()
        .await
        .context("failed to commit conversation transaction")?;
    Ok(id)
}

pub async fn prune(db: &SqlitePool, older_than: i64) -> Result<u64> {
    Ok(
        sqlx::query("DELETE FROM conversations WHERE updated_at < ?")
            .bind(older_than)
            .execute(db)
            .await
            .context("failed to prune old conversations")?
            .rows_affected(),
    )
}

pub fn spawn_pruning(db: SqlitePool, retention_days: u32) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(24 * 60 * 60));
        loop {
            interval.tick().await;
            let cutoff =
                serenity::Timestamp::now().unix_timestamp() - i64::from(retention_days) * 86_400;
            match prune(&db, cutoff).await {
                Ok(count) => info!(count, "pruned old conversations"),
                Err(error) => error!(?error, "failed to prune old conversations"),
            }
        }
    });
}

fn strip_images(transcript: &[Message]) -> Vec<Message> {
    transcript
        .iter()
        .cloned()
        .map(|message| match message {
            Message::User {
                content: UserContent::Parts(parts),
            } => Message::User {
                content: UserContent::from_parts(
                    parts
                        .into_iter()
                        .map(|part| match part {
                            ContentPart::ImageUrl { .. } => ContentPart::Text {
                                text: "[image omitted from saved history]".to_owned(),
                            },
                            part => part,
                        })
                        .collect(),
                ),
            },
            message => message,
        })
        .collect()
}

fn discord_id(id: u64) -> Result<i64> {
    i64::try_from(id).context("Discord ID exceeds SQLite INTEGER range")
}

#[cfg(test)]
mod tests;
