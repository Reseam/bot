use anyhow::{Context, Result, bail};
use poise::serenity_prelude as serenity;
use sqlx::SqlitePool;

use super::{post_log, require_channel_permission};
use crate::db::discord_id;
use crate::discord::{History, is_thread};
use crate::moderation::{
    Action, Moderator, PurgeCandidate, Record, SavedOverwrite, is_staff_role, lock_plan,
    purge_matches,
};

const BULK_DELETE_AGE: i64 = 14 * 24 * 60 * 60;
const MAX_SCANNED: usize = 1_000;

pub async fn delete_message(
    moderator: &Moderator<'_>,
    channel_id: serenity::ChannelId,
    message_id: serenity::MessageId,
    reason: &str,
) -> Result<()> {
    require_channel_permission(
        moderator,
        channel_id,
        serenity::Permissions::MANAGE_MESSAGES,
    )
    .await?;
    let message = channel_id
        .message(moderator.discord, message_id)
        .await
        .context("failed to fetch message")?;
    channel_id
        .delete_message(moderator.discord, message_id)
        .await
        .context("failed to delete message")?;
    log(
        moderator,
        Action::Delete,
        Some(message.author.id),
        channel_id,
        &moderator.reason(reason),
    )
    .await;
    Ok(())
}

pub async fn purge(
    moderator: &Moderator<'_>,
    channel_id: serenity::ChannelId,
    count: usize,
    user: Option<serenity::UserId>,
    contains: Option<&str>,
    bots: Option<bool>,
) -> Result<usize> {
    require_channel_permission(
        moderator,
        channel_id,
        serenity::Permissions::MANAGE_MESSAGES | serenity::Permissions::READ_MESSAGE_HISTORY,
    )
    .await?;
    let cutoff = serenity::Timestamp::now().unix_timestamp() - BULK_DELETE_AGE;
    let mut history = History::before(channel_id, None);
    let mut matched = Vec::new();
    let mut scanned = 0;
    'pages: while scanned < MAX_SCANNED {
        let page = history.next_page(moderator.discord).await?;
        if page.is_empty() {
            break;
        }
        for message in page {
            scanned += 1;
            if message.timestamp.unix_timestamp() <= cutoff {
                break 'pages;
            }
            let candidate = PurgeCandidate {
                author: message.author.id,
                author_is_bot: message.author.bot,
                content: &message.content,
            };
            if purge_matches(&candidate, user, contains, bots) {
                matched.push(message.id);
                if matched.len() == count {
                    break 'pages;
                }
            }
        }
    }
    for chunk in matched.chunks(100) {
        channel_id
            .delete_messages(moderator.discord, chunk)
            .await
            .context("failed to delete messages")?;
    }
    log(
        moderator,
        Action::Purge,
        user,
        channel_id,
        &moderator.reason(&format!("Deleted {} messages", matched.len())),
    )
    .await;
    Ok(matched.len())
}

pub async fn set_slowmode(
    moderator: &Moderator<'_>,
    channel_id: serenity::ChannelId,
    seconds: u16,
) -> Result<()> {
    require_channel_permission(
        moderator,
        channel_id,
        serenity::Permissions::MANAGE_CHANNELS,
    )
    .await?;
    channel_id
        .edit(
            moderator.discord,
            serenity::EditChannel::new().rate_limit_per_user(seconds),
        )
        .await
        .context("failed to update channel slowmode")?;
    log(
        moderator,
        Action::Slowmode,
        None,
        channel_id,
        &moderator.reason(&format!("Set slowmode to {seconds} seconds")),
    )
    .await;
    Ok(())
}

pub async fn lock(
    moderator: &Moderator<'_>,
    channel_id: serenity::ChannelId,
    reason: &str,
) -> Result<()> {
    let channel = require_channel_permission(
        moderator,
        channel_id,
        serenity::Permissions::MANAGE_CHANNELS,
    )
    .await?;
    if is_thread(channel.kind) {
        bail!("threads cannot be locked this way; lock the parent channel instead");
    }
    if saved_overwrites(moderator.db, channel_id).await?.is_some() {
        bail!("#{} is already locked", channel.name);
    }
    let everyone = moderator.guild_id.everyone_role();
    let plan = {
        let guild = moderator
            .discord
            .cache
            .guild(moderator.guild_id)
            .context("server is not available in the Discord cache")?;
        lock_plan(&channel.permission_overwrites, everyone, |role| {
            guild
                .roles
                .get(&role)
                .is_some_and(|role| is_staff_role(role.permissions))
        })
    };
    let saved = plan.iter().map(|(saved, _)| saved).collect::<Vec<_>>();
    sqlx::query("INSERT INTO channel_locks (channel_id, guild_id, overwrites) VALUES (?, ?, ?)")
        .bind(discord_id(channel_id.get())?)
        .bind(discord_id(moderator.guild_id.get())?)
        .bind(serde_json::to_string(&saved).context("failed to serialize channel permissions")?)
        .execute(moderator.db)
        .await
        .context("failed to save channel permissions")?;
    for (_, overwrite) in plan {
        channel_id
            .create_permission(moderator.discord, overwrite)
            .await
            .context("failed to update channel permissions")?;
    }
    log(
        moderator,
        Action::Lock,
        None,
        channel_id,
        &moderator.reason(reason),
    )
    .await;
    Ok(())
}

pub async fn unlock(
    moderator: &Moderator<'_>,
    channel_id: serenity::ChannelId,
    reason: &str,
) -> Result<()> {
    let channel = require_channel_permission(
        moderator,
        channel_id,
        serenity::Permissions::MANAGE_CHANNELS,
    )
    .await?;
    let saved = saved_overwrites(moderator.db, channel_id)
        .await?
        .with_context(|| format!("#{} was not locked by the bot", channel.name))?;
    for overwrite in saved {
        let role = serenity::RoleId::new(overwrite.role);
        match overwrite.original {
            Some((allow, deny)) => {
                channel_id
                    .create_permission(
                        moderator.discord,
                        serenity::PermissionOverwrite {
                            allow: serenity::Permissions::from_bits_retain(allow),
                            deny: serenity::Permissions::from_bits_retain(deny),
                            kind: serenity::PermissionOverwriteType::Role(role),
                        },
                    )
                    .await
            }
            None => {
                channel_id
                    .delete_permission(
                        moderator.discord,
                        serenity::PermissionOverwriteType::Role(role),
                    )
                    .await
            }
        }
        .context("failed to restore channel permissions")?;
    }
    sqlx::query("DELETE FROM channel_locks WHERE channel_id = ?")
        .bind(discord_id(channel_id.get())?)
        .execute(moderator.db)
        .await
        .context("failed to clear channel lock")?;
    log(
        moderator,
        Action::Unlock,
        None,
        channel_id,
        &moderator.reason(reason),
    )
    .await;
    Ok(())
}

async fn saved_overwrites(
    db: &SqlitePool,
    channel_id: serenity::ChannelId,
) -> Result<Option<Vec<SavedOverwrite>>> {
    sqlx::query_scalar::<_, String>("SELECT overwrites FROM channel_locks WHERE channel_id = ?")
        .bind(discord_id(channel_id.get())?)
        .fetch_optional(db)
        .await
        .context("failed to read channel lock")?
        .map(|json| serde_json::from_str(&json).context("invalid saved channel permissions"))
        .transpose()
}

async fn log(
    moderator: &Moderator<'_>,
    action: Action,
    target: Option<serenity::UserId>,
    channel_id: serenity::ChannelId,
    reason: &str,
) {
    post_log(
        moderator.discord,
        moderator.db,
        moderator.guild_id,
        moderator.actor.user.id,
        &Record {
            action,
            target,
            channel: Some(channel_id),
            reason,
            duration: None,
            expires_at: None,
        },
    )
    .await;
}
