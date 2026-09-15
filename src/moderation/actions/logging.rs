use std::time::Duration;

use poise::serenity_prelude as serenity;
use tracing::warn;

use crate::moderation::Case;

pub async fn post_log(discord: &serenity::Context, db: &sqlx::SqlitePool, case: &Case) {
    let channel = match sqlx::query_scalar::<_, Option<i64>>(
        "SELECT mod_log_channel_id FROM guild_settings WHERE guild_id = ?",
    )
    .bind(case.guild_id)
    .fetch_optional(db)
    .await
    {
        Ok(Some(Some(id))) => crate::db::stored_discord_id(id)
            .ok()
            .map(serenity::ChannelId::new),
        Ok(_) => None,
        Err(error) => {
            warn!(
                ?error,
                case_id = case.id,
                "failed to read moderation log setting"
            );
            None
        }
    };
    let Some(channel) = channel else { return };
    let duration = case.duration_secs.map_or_else(
        || "None".to_owned(),
        |seconds| humantime::format_duration(Duration::from_secs(seconds as u64)).to_string(),
    );
    let expiry = case.expires_at.map_or_else(
        || "None".to_owned(),
        |timestamp| format!("<t:{timestamp}:F>"),
    );
    let target = [
        case.target_id.map(|id| format!("Member: <@{id}> (`{id}`)")),
        case.channel_id
            .map(|id| format!("Channel: <#{id}> (`{id}`)")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n");
    let embed = serenity::CreateEmbed::new()
        .title(format!("Moderation case #{}", case.id))
        .field("Action", case.action.to_string(), true)
        .field("Target", target, false)
        .field("Moderator", format!("<@{}>", case.moderator_id), true)
        .field("Reason", &case.reason, false)
        .field("Duration", duration, true)
        .field("Expiry", expiry, true)
        .timestamp(serenity::Timestamp::from_unix_timestamp(case.created_at).unwrap_or_default());
    if let Err(error) = channel
        .send_message(
            discord,
            serenity::CreateMessage::new()
                .embed(embed)
                .allowed_mentions(serenity::CreateAllowedMentions::new()),
        )
        .await
    {
        warn!(?error, case_id = case.id, %channel, "failed to post moderation log");
    }
}
