use std::time::Duration;

use anyhow::{Context, Result};
use poise::serenity_prelude as serenity;

use super::{Action, Moderator, NewCase, Outcome, finish, require_actor_channel_permission};
use crate::db::discord_id;

const MESSAGE_AGE_LIMIT: i64 = 14 * 24 * 60 * 60;

pub async fn delete_message(
    moderator: &Moderator<'_>,
    channel_id: serenity::ChannelId,
    message_id: serenity::MessageId,
    reason: &str,
) -> Result<Outcome> {
    require_actor_channel_permission(
        moderator,
        channel_id,
        serenity::Permissions::MANAGE_MESSAGES,
    )?;
    let message = channel_id
        .message(moderator.discord, message_id)
        .await
        .context("failed to fetch message")?;
    channel_id
        .delete_message(moderator.discord, message_id)
        .await
        .context("failed to delete message")?;
    let reason = moderator.reason(reason);
    finish(
        moderator,
        NewCase {
            action: Action::Purge,
            target_id: Some(message.author.id),
            channel_id: Some(channel_id),
            reason: &reason,
            duration: None,
            expires_at: None,
            dm_delivered: None,
        },
    )
    .await
}

pub async fn record_purge(
    moderator: &Moderator<'_>,
    target: Option<serenity::UserId>,
    channel_id: serenity::ChannelId,
    count: usize,
) -> Result<Outcome> {
    let reason = moderator.reason(&format!("Deleted {count} messages"));
    finish(
        moderator,
        NewCase {
            action: Action::Purge,
            target_id: target,
            channel_id: Some(channel_id),
            reason: &reason,
            duration: None,
            expires_at: None,
            dm_delivered: None,
        },
    )
    .await
}

pub async fn set_slowmode(
    moderator: &Moderator<'_>,
    channel: &serenity::GuildChannel,
    seconds: u16,
) -> Result<Outcome> {
    require_actor_channel_permission(
        moderator,
        channel.id,
        serenity::Permissions::MANAGE_CHANNELS,
    )?;
    channel
        .id
        .edit(
            moderator.discord,
            serenity::EditChannel::new().rate_limit_per_user(seconds),
        )
        .await
        .context("failed to update channel slowmode")?;
    let reason = moderator.reason(&format!("Set slowmode to {seconds} seconds"));
    finish(
        moderator,
        NewCase {
            action: Action::Slowmode,
            target_id: None,
            channel_id: Some(channel.id),
            reason: &reason,
            duration: Some(Duration::from_secs(u64::from(seconds))),
            expires_at: None,
            dm_delivered: None,
        },
    )
    .await
}

pub async fn set_locked(
    moderator: &Moderator<'_>,
    channel: &serenity::GuildChannel,
    locked: bool,
    reason: &str,
) -> Result<Outcome> {
    require_actor_channel_permission(
        moderator,
        channel.id,
        serenity::Permissions::MANAGE_CHANNELS,
    )?;
    let everyone = moderator.guild_id.everyone_role();
    let current = channel
        .permission_overwrites
        .iter()
        .find(|overwrite| overwrite.kind == serenity::PermissionOverwriteType::Role(everyone));
    let overwrite = crate::moderation::update_everyone_overwrite(current, everyone, locked);
    channel
        .id
        .create_permission(moderator.discord, overwrite)
        .await
        .context("failed to update channel permission overwrite")?;
    let reason = moderator.reason(reason);
    finish(
        moderator,
        NewCase {
            action: if locked { Action::Lock } else { Action::Unlock },
            target_id: None,
            channel_id: Some(channel.id),
            reason: &reason,
            duration: None,
            expires_at: None,
            dm_delivered: None,
        },
    )
    .await
}

pub async fn purge_messages(
    moderator: &Moderator<'_>,
    channel_id: serenity::ChannelId,
    count: u8,
    user: Option<serenity::UserId>,
    contains: Option<&str>,
    bots: Option<bool>,
) -> Result<usize> {
    require_actor_channel_permission(
        moderator,
        channel_id,
        serenity::Permissions::MANAGE_MESSAGES,
    )?;
    let cutoff = serenity::Timestamp::from_unix_timestamp(
        serenity::Timestamp::now().unix_timestamp() - MESSAGE_AGE_LIMIT,
    )
    .context("message age cutoff is invalid")?;
    let messages = channel_id
        .messages(moderator.discord, serenity::GetMessages::new().limit(100))
        .await
        .context("failed to fetch messages for purge")?;
    let ids = messages
        .iter()
        .filter(|message| {
            crate::moderation::purge_matches(
                crate::moderation::PurgeCandidate {
                    timestamp: message.timestamp,
                    author: message.author.id,
                    author_is_bot: message.author.bot,
                    content: &message.content,
                },
                user,
                contains,
                bots,
                cutoff,
            )
        })
        .take(usize::from(count))
        .map(|message| message.id)
        .collect::<Vec<_>>();
    if !ids.is_empty() {
        channel_id
            .delete_messages(moderator.discord, &ids)
            .await
            .context("failed to purge messages")?;
    }
    Ok(ids.len())
}

pub async fn set_mod_log(
    db: &sqlx::SqlitePool,
    guild_id: serenity::GuildId,
    channel: serenity::ChannelId,
) -> Result<()> {
    sqlx::query(
        "INSERT INTO guild_settings (guild_id, mod_log_channel_id) VALUES (?, ?) \
         ON CONFLICT(guild_id) DO UPDATE SET mod_log_channel_id = excluded.mod_log_channel_id",
    )
    .bind(discord_id(guild_id.get())?)
    .bind(discord_id(channel.get())?)
    .execute(db)
    .await
    .context("failed to save moderation log channel")?;
    Ok(())
}
