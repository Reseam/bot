use std::time::Duration;

use anyhow::{Context, Result, bail};
use poise::serenity_prelude as serenity;

use super::{Action, Moderator, Record, hierarchy_allows, set_temp_ban};
use crate::discord::{UNKNOWN_MEMBER, error_code, resolve_channel};

mod channel;
mod logging;

pub use channel::{
    delete_message, lock, mod_log_channel, purge, set_mod_log, set_slowmode, unlock,
};
pub use logging::post_log;

pub struct Outcome {
    dm_delivered: Option<bool>,
}

impl Outcome {
    pub fn describe(&self, verb: &str, target: serenity::UserId) -> String {
        let dm = match self.dm_delivered {
            Some(true) => " DM delivered.",
            Some(false) => " DM failed.",
            None => "",
        };
        format!("{verb} <@{target}>.{dm}")
    }
}

pub async fn member(
    moderator: &Moderator<'_>,
    user_id: serenity::UserId,
) -> Result<Option<serenity::Member>> {
    match moderator.guild_id.member(moderator.discord, user_id).await {
        Ok(member) => Ok(Some(member)),
        Err(error) if error_code(&error) == Some(UNKNOWN_MEMBER) => Ok(None),
        Err(error) => Err(error).context("failed to fetch target member"),
    }
}

pub async fn warn(
    moderator: &Moderator<'_>,
    target: &serenity::Member,
    reason: &str,
) -> Result<Outcome> {
    validate_target(
        moderator,
        target.user.id,
        Some(target),
        serenity::Permissions::MODERATE_MEMBERS,
    )?;
    let reason = moderator.reason(reason);
    let dm = send_dm(moderator, target, Action::Warn, None, &reason).await;
    log(moderator, Action::Warn, target.user.id, &reason, None, None).await;
    Ok(Outcome {
        dm_delivered: Some(dm),
    })
}

pub async fn timeout(
    moderator: &Moderator<'_>,
    target: &serenity::Member,
    duration: Duration,
    reason: &str,
) -> Result<Outcome> {
    validate_target(
        moderator,
        target.user.id,
        Some(target),
        serenity::Permissions::MODERATE_MEMBERS,
    )?;
    let reason = moderator.reason(reason);
    let expires_at = serenity::Timestamp::now().unix_timestamp()
        + i64::try_from(duration.as_secs()).context("duration is too large")?;
    moderator
        .guild_id
        .edit_member(
            moderator.discord,
            target.user.id,
            serenity::EditMember::new()
                .disable_communication_until_datetime(
                    serenity::Timestamp::from_unix_timestamp(expires_at)
                        .context("timeout expiry is outside Discord's timestamp range")?,
                )
                .audit_log_reason(&reason),
        )
        .await
        .context("failed to timeout member")?;
    let dm = send_dm(moderator, target, Action::Timeout, Some(duration), &reason).await;
    log(
        moderator,
        Action::Timeout,
        target.user.id,
        &reason,
        Some(duration),
        Some(expires_at),
    )
    .await;
    Ok(Outcome {
        dm_delivered: Some(dm),
    })
}

pub async fn untimeout(
    moderator: &Moderator<'_>,
    target: &serenity::Member,
    reason: &str,
) -> Result<Outcome> {
    validate_target(
        moderator,
        target.user.id,
        Some(target),
        serenity::Permissions::MODERATE_MEMBERS,
    )?;
    let reason = moderator.reason(reason);
    moderator
        .guild_id
        .edit_member(
            moderator.discord,
            target.user.id,
            serenity::EditMember::new()
                .enable_communication()
                .audit_log_reason(&reason),
        )
        .await
        .context("failed to remove member timeout")?;
    log(
        moderator,
        Action::Untimeout,
        target.user.id,
        &reason,
        None,
        None,
    )
    .await;
    Ok(Outcome { dm_delivered: None })
}

pub async fn kick(
    moderator: &Moderator<'_>,
    target: &serenity::Member,
    reason: &str,
) -> Result<Outcome> {
    validate_target(
        moderator,
        target.user.id,
        Some(target),
        serenity::Permissions::KICK_MEMBERS,
    )?;
    let reason = moderator.reason(reason);
    let dm = send_dm(moderator, target, Action::Kick, None, &reason).await;
    moderator
        .guild_id
        .kick_with_reason(moderator.discord, target.user.id, &reason)
        .await
        .context("failed to kick member")?;
    log(moderator, Action::Kick, target.user.id, &reason, None, None).await;
    Ok(Outcome {
        dm_delivered: Some(dm),
    })
}

pub async fn ban(
    moderator: &Moderator<'_>,
    user_id: serenity::UserId,
    target: Option<&serenity::Member>,
    reason: &str,
    duration: Option<Duration>,
    delete_message_days: u8,
) -> Result<Outcome> {
    validate_target(
        moderator,
        user_id,
        target,
        serenity::Permissions::BAN_MEMBERS,
    )?;
    let reason = moderator.reason(reason);
    let expires_at = duration
        .map(|value| i64::try_from(value.as_secs()).context("duration is too large"))
        .transpose()?
        .map(|seconds| serenity::Timestamp::now().unix_timestamp() + seconds);
    let dm = match target {
        Some(target) => Some(send_dm(moderator, target, Action::Ban, duration, &reason).await),
        None => None,
    };
    moderator
        .guild_id
        .ban_with_reason(moderator.discord, user_id, delete_message_days, &reason)
        .await
        .context("failed to ban user")?;
    set_temp_ban(moderator.db, moderator.guild_id, user_id, expires_at).await?;
    log(
        moderator,
        Action::Ban,
        user_id,
        &reason,
        duration,
        expires_at,
    )
    .await;
    Ok(Outcome { dm_delivered: dm })
}

pub async fn unban(
    moderator: &Moderator<'_>,
    user_id: serenity::UserId,
    reason: &str,
) -> Result<Outcome> {
    validate_target(moderator, user_id, None, serenity::Permissions::BAN_MEMBERS)?;
    let reason = moderator.reason(reason);
    moderator
        .guild_id
        .unban(moderator.discord, user_id)
        .await
        .context("failed to unban user")?;
    set_temp_ban(moderator.db, moderator.guild_id, user_id, None).await?;
    log(moderator, Action::Unban, user_id, &reason, None, None).await;
    Ok(Outcome { dm_delivered: None })
}

pub fn validate_target(
    moderator: &Moderator<'_>,
    user_id: serenity::UserId,
    target: Option<&serenity::Member>,
    required: serenity::Permissions,
) -> Result<()> {
    let guild = moderator
        .discord
        .cache
        .guild(moderator.guild_id)
        .context("server is not available in the Discord cache")?;
    let bot_id = moderator.discord.cache.current_user().id;
    if user_id == guild.owner_id {
        bail!("the server owner cannot be moderated");
    }
    if user_id == moderator.actor.user.id {
        bail!("you cannot moderate yourself");
    }
    if user_id == bot_id {
        bail!("the bot cannot moderate itself");
    }
    if !guild
        .member_permissions(&moderator.actor)
        .contains(required)
    {
        bail!(
            "invoker is missing {}",
            required.get_permission_names().join(", ")
        );
    }
    let Some(target) = target else {
        return Ok(());
    };
    let bot = guild
        .members
        .get(&bot_id)
        .context("bot member is not available in the Discord cache")?;
    let target_position = highest_role_position(&guild, target);
    if !hierarchy_allows(
        highest_role_position(&guild, &moderator.actor),
        target_position,
        moderator.actor.user.id == guild.owner_id,
    ) {
        bail!("your highest role must be above the target's highest role");
    }
    if highest_role_position(&guild, bot) <= target_position {
        bail!("the bot's highest role must be above the target's highest role");
    }
    Ok(())
}

pub async fn require_channel_permission(
    moderator: &Moderator<'_>,
    channel_id: serenity::ChannelId,
    required: serenity::Permissions,
) -> Result<serenity::GuildChannel> {
    let access = resolve_channel(
        moderator.discord,
        moderator.guild_id,
        &moderator.actor,
        channel_id,
    )
    .await?;
    if !access.permissions.contains(required) {
        bail!(
            "invoker is missing {} in this channel",
            required.get_permission_names().join(", ")
        );
    }
    Ok(access.channel)
}

fn highest_role_position(guild: &serenity::Guild, member: &serenity::Member) -> u16 {
    member
        .roles
        .iter()
        .filter_map(|id| guild.roles.get(id))
        .map(|role| role.position)
        .max()
        .unwrap_or_default()
}

async fn send_dm(
    moderator: &Moderator<'_>,
    target: &serenity::Member,
    action: Action,
    duration: Option<Duration>,
    reason: &str,
) -> bool {
    let guild_name = moderator
        .discord
        .cache
        .guild(moderator.guild_id)
        .map_or_else(|| "this server".to_owned(), |guild| guild.name.clone());
    let duration = duration.map_or_else(String::new, |value| {
        format!("\nDuration: {}", humantime::format_duration(value))
    });
    target
        .user
        .direct_message(
            moderator.discord,
            serenity::CreateMessage::new()
                .content(format!(
                    "Moderation notice from {guild_name}\nAction: {action}{duration}\nReason: {reason}"
                ))
                .allowed_mentions(serenity::CreateAllowedMentions::new()),
        )
        .await
        .is_ok()
}

async fn log(
    moderator: &Moderator<'_>,
    action: Action,
    target: serenity::UserId,
    reason: &str,
    duration: Option<Duration>,
    expires_at: Option<i64>,
) {
    post_log(
        moderator.discord,
        moderator.db,
        moderator.guild_id,
        moderator.actor.user.id,
        &Record {
            action,
            target: Some(target),
            channel: None,
            reason,
            duration,
            expires_at,
        },
    )
    .await;
}
