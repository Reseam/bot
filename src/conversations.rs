use std::time::Duration;

use anyhow::{Context, Result};
use poise::serenity_prelude as serenity;
use sqlx::{FromRow, SqlitePool};
use tracing::{error, info};

use crate::db::{discord_id, stored_discord_id};
use crate::llm::{ContentPart, Message, UserContent};

pub struct Conversation {
    pub transcript: Vec<Message>,
    pub last_message_id: Option<serenity::MessageId>,
    pub prompt_prefix: Option<String>,
    pub sandbox_image: Option<String>,
}

#[derive(FromRow)]
struct ConversationRow {
    transcript: String,
    last_message_id: Option<i64>,
    prompt_prefix: Option<String>,
    sandbox_image: Option<String>,
}

pub async fn create(
    db: &SqlitePool,
    guild_id: serenity::GuildId,
    channel_id: serenity::ChannelId,
    started_by: serenity::UserId,
) -> Result<i64> {
    let now = serenity::Timestamp::now().unix_timestamp();
    Ok(sqlx::query(
        "INSERT INTO conversations \
         (guild_id, channel_id, started_by, transcript, created_at, updated_at) \
         VALUES (?, ?, ?, '[]', ?, ?)",
    )
    .bind(discord_id(guild_id.get())?)
    .bind(discord_id(channel_id.get())?)
    .bind(discord_id(started_by.get())?)
    .bind(now)
    .bind(now)
    .execute(db)
    .await
    .context("failed to create conversation")?
    .last_insert_rowid())
}

pub async fn find_by_message(
    db: &SqlitePool,
    message_id: serenity::MessageId,
) -> Result<Option<i64>> {
    sqlx::query_scalar("SELECT conversation_id FROM conversation_messages WHERE message_id = ?")
        .bind(discord_id(message_id.get())?)
        .fetch_optional(db)
        .await
        .context("failed to find conversation by Discord message")
}

pub async fn load(db: &SqlitePool, id: i64) -> Result<Conversation> {
    let row = sqlx::query_as::<_, ConversationRow>(
        "SELECT transcript, prompt_prefix, sandbox_image, \
         (SELECT MAX(message_id) FROM conversation_messages WHERE conversation_id = c.id) \
         AS last_message_id \
         FROM conversations c WHERE id = ?",
    )
    .bind(id)
    .fetch_one(db)
    .await
    .context("failed to load conversation")?;
    Ok(Conversation {
        transcript: serde_json::from_str(&row.transcript)
            .context("failed to deserialize conversation transcript")?,
        last_message_id: row
            .last_message_id
            .map(|id| stored_discord_id(id).map(serenity::MessageId::new))
            .transpose()?,
        prompt_prefix: row.prompt_prefix,
        sandbox_image: row.sandbox_image,
    })
}

pub async fn save(
    db: &SqlitePool,
    id: i64,
    transcript: &[Message],
    prompt_prefix: &str,
    message_ids: &[serenity::MessageId],
) -> Result<()> {
    let transcript = serde_json::to_string(&strip_images(transcript))
        .context("failed to serialize conversation transcript")?;
    let mut transaction = db
        .begin()
        .await
        .context("failed to begin conversation transaction")?;
    sqlx::query(
        "UPDATE conversations SET transcript = ?, prompt_prefix = ?, updated_at = ? WHERE id = ?",
    )
    .bind(&transcript)
    .bind(prompt_prefix)
    .bind(serenity::Timestamp::now().unix_timestamp())
    .bind(id)
    .execute(&mut *transaction)
    .await
    .context("failed to update conversation")?;
    for message_id in message_ids {
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
        .context("failed to commit conversation transaction")
}

pub async fn set_sandbox_image(db: &SqlitePool, id: i64, image: &str) -> Result<()> {
    sqlx::query("UPDATE conversations SET sandbox_image = ? WHERE id = ?")
        .bind(image)
        .bind(id)
        .execute(db)
        .await
        .context("failed to save sandbox image")?;
    Ok(())
}

pub fn spawn_pruning(db: SqlitePool, retention_days: u32) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(Duration::from_secs(24 * 60 * 60));
        loop {
            interval.tick().await;
            let cutoff =
                serenity::Timestamp::now().unix_timestamp() - i64::from(retention_days) * 86_400;
            match sqlx::query("DELETE FROM conversations WHERE updated_at < ?")
                .bind(cutoff)
                .execute(&db)
                .await
            {
                Ok(result) => info!(count = result.rows_affected(), "pruned old conversations"),
                Err(error) => error!(?error, "failed to prune old conversations"),
            }
        }
    });
}

fn strip_images(transcript: &[Message]) -> Vec<Message> {
    let mut stripped = false;
    transcript
        .iter()
        .cloned()
        .map(|message| match message {
            Message::User {
                content: UserContent::Parts(parts),
            } if parts
                .iter()
                .any(|part| matches!(part, ContentPart::ImageUrl { .. })) =>
            {
                stripped = true;
                Message::User {
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
                }
            }
            // Replay blocks after a stripped image were bound to its bytes.
            Message::Assistant(mut message) if stripped => {
                message.anthropic_content.clear();
                Message::Assistant(message)
            }
            message => message,
        })
        .collect()
}

#[cfg(test)]
mod tests;
