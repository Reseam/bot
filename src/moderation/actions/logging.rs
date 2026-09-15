use std::time::Duration;

use poise::serenity_prelude as serenity;
use sqlx::SqlitePool;
use tracing::warn;

use crate::moderation::Record;
use crate::settings::mod_log_channel;

pub async fn post_log(
    discord: &serenity::Context,
    db: &SqlitePool,
    guild_id: serenity::GuildId,
    moderator_id: serenity::UserId,
    record: &Record<'_>,
) {
    let channel = match mod_log_channel(db, guild_id).await {
        Ok(Some(channel)) => channel,
        Ok(None) => return,
        Err(error) => {
            warn!(error = %format!("{error:#}"), %guild_id, "failed to read moderation log setting");
            return;
        }
    };
    let target = [
        record.target.map(|id| format!("Member: <@{id}> (`{id}`)")),
        record
            .channel
            .map(|id| format!("Channel: <#{id}> (`{id}`)")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n");
    let mut embed = serenity::CreateEmbed::new()
        .title(record.action.to_string())
        .field("Moderator", format!("<@{moderator_id}>"), true)
        .field("Reason", record.reason, false)
        .timestamp(serenity::Timestamp::now());
    if !target.is_empty() {
        embed = embed.field("Target", target, false);
    }
    if let Some(duration) = record.duration {
        embed = embed.field(
            "Duration",
            humantime::format_duration(Duration::from_secs(duration.as_secs())).to_string(),
            true,
        );
    }
    if let Some(expires_at) = record.expires_at {
        embed = embed.field("Expires", format!("<t:{expires_at}:F>"), true);
    }
    if let Err(error) = channel
        .send_message(
            discord,
            serenity::CreateMessage::new()
                .embed(embed)
                .allowed_mentions(serenity::CreateAllowedMentions::new()),
        )
        .await
    {
        warn!(?error, %channel, "failed to post moderation log");
    }
}
