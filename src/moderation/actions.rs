use std::time::Duration;

use super::{Action, Case, Moderator, NewCase, hierarchy_allows, insert_case};
use anyhow::{Context, Result, bail};
use poise::serenity_prelude as serenity;

mod channel;
mod logging;

pub use channel::{
    delete_message, purge_messages, record_purge, set_locked, set_mod_log, set_slowmode,
};
pub use logging::post_log;

pub struct Outcome {
    pub case: Case,
    pub dm_delivered: Option<bool>,
}

pub async fn warn(
    moderator: &Moderator<'_>,
    target: &serenity::Member,
    reason: &str,
) -> Result<Outcome> {
    validate_target(moderator, target, serenity::Permissions::MODERATE_MEMBERS)?;
    let reason = moderator.reason(reason);
    let dm = send_dm(moderator, target, Action::Warn, None, &reason).await;
    finish(
        moderator,
        NewCase {
            action: Action::Warn,
            target_id: Some(target.user.id),
            channel_id: None,
            reason: &reason,
            duration: None,
            expires_at: None,
            dm_delivered: Some(dm),
        },
    )
    .await
}

pub async fn note(
    moderator: &Moderator<'_>,
    target: &serenity::Member,
    text: &str,
) -> Result<Outcome> {
    validate_target(moderator, target, serenity::Permissions::MODERATE_MEMBERS)?;
    let reason = moderator.reason(text);
    finish(
        moderator,
        NewCase {
            action: Action::Note,
            target_id: Some(target.user.id),
            channel_id: None,
            reason: &reason,
            duration: None,
            expires_at: None,
            dm_delivered: None,
        },
    )
    .await
}

pub async fn timeout(
    moderator: &Moderator<'_>,
    target: &serenity::Member,
    duration: Duration,
    reason: &str,
) -> Result<Outcome> {
    validate_target(moderator, target, serenity::Permissions::MODERATE_MEMBERS)?;
    let reason = moderator.reason(reason);
    let dm = send_dm(moderator, target, Action::Timeout, Some(duration), &reason).await;
    let expires_at = serenity::Timestamp::now().unix_timestamp()
        + i64::try_from(duration.as_secs()).context("duration is too large")?;
    moderator
        .guild_id
        .edit_member(
            moderator.discord,
            target.user.id,
            serenity::EditMember::new().disable_communication_until_datetime(
                serenity::Timestamp::from_unix_timestamp(expires_at)
                    .context("timeout expiry is outside Discord's timestamp range")?,
            ),
        )
        .await
        .context("failed to timeout member")?;
    finish(
        moderator,
        NewCase {
            action: Action::Timeout,
            target_id: Some(target.user.id),
            channel_id: None,
            reason: &reason,
            duration: Some(duration),
            expires_at: Some(expires_at),
            dm_delivered: Some(dm),
        },
    )
    .await
}

pub async fn untimeout(
    moderator: &Moderator<'_>,
    target: &serenity::Member,
    reason: &str,
) -> Result<Outcome> {
    validate_target(moderator, target, serenity::Permissions::MODERATE_MEMBERS)?;
    let reason = moderator.reason(reason);
    moderator
        .guild_id
        .edit_member(
            moderator.discord,
            target.user.id,
            serenity::EditMember::new().enable_communication(),
        )
        .await
        .context("failed to remove member timeout")?;
    finish(
        moderator,
        NewCase {
            action: Action::Untimeout,
            target_id: Some(target.user.id),
            channel_id: None,
            reason: &reason,
            duration: None,
            expires_at: None,
            dm_delivered: None,
        },
    )
    .await
}

pub async fn kick(
    moderator: &Moderator<'_>,
    target: &serenity::Member,
    reason: &str,
) -> Result<Outcome> {
    validate_target(moderator, target, serenity::Permissions::KICK_MEMBERS)?;
    let reason = moderator.reason(reason);
    let dm = send_dm(moderator, target, Action::Kick, None, &reason).await;
    moderator
        .guild_id
        .kick_with_reason(moderator.discord, target.user.id, &reason)
        .await
        .context("failed to kick member")?;
    finish(
        moderator,
        NewCase {
            action: Action::Kick,
            target_id: Some(target.user.id),
            channel_id: None,
            reason: &reason,
            duration: None,
            expires_at: None,
            dm_delivered: Some(dm),
        },
    )
    .await
}

pub async fn ban(
    moderator: &Moderator<'_>,
    target: &serenity::Member,
    reason: &str,
    duration: Option<Duration>,
    delete_message_days: u8,
) -> Result<Outcome> {
    validate_target(moderator, target, serenity::Permissions::BAN_MEMBERS)?;
    let reason = moderator.reason(reason);
    let dm = send_dm(moderator, target, Action::Ban, duration, &reason).await;
    moderator
        .guild_id
        .ban_with_reason(
            moderator.discord,
            target.user.id,
            delete_message_days,
            &reason,
        )
        .await
        .context("failed to ban member")?;
    let expires_at = duration
        .map(|value| i64::try_from(value.as_secs()).context("duration is too large"))
        .transpose()?
        .map(|seconds| serenity::Timestamp::now().unix_timestamp() + seconds);
    finish(
        moderator,
        NewCase {
            action: Action::Ban,
            target_id: Some(target.user.id),
            channel_id: None,
            reason: &reason,
            duration,
            expires_at,
            dm_delivered: Some(dm),
        },
    )
    .await
}

pub async fn unban(
    moderator: &Moderator<'_>,
    target: serenity::UserId,
    reason: &str,
) -> Result<Outcome> {
    require_actor_permission(moderator, serenity::Permissions::BAN_MEMBERS)?;
    let reason = moderator.reason(reason);
    moderator
        .guild_id
        .unban(moderator.discord, target)
        .await
        .context("failed to unban user")?;
    finish(
        moderator,
        NewCase {
            action: Action::Unban,
            target_id: Some(target),
            channel_id: None,
            reason: &reason,
            duration: None,
            expires_at: None,
            dm_delivered: None,
        },
    )
    .await
}

fn validate_target(
    moderator: &Moderator<'_>,
    target: &serenity::Member,
    required: serenity::Permissions,
) -> Result<()> {
    let guild = moderator
        .discord
        .cache
        .guild(moderator.guild_id)
        .context("server is not available in the Discord cache")?;
    let bot_id = moderator.discord.cache.current_user().id;
    if target.user.id == guild.owner_id {
        bail!("the server owner cannot be moderated");
    }
    if target.user.id == moderator.actor.user.id {
        bail!("you cannot moderate yourself");
    }
    if target.user.id == bot_id {
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
    let bot = guild
        .members
        .get(&bot_id)
        .context("bot member is not available in the Discord cache")?;
    let actor_position = highest_role_position(&guild, &moderator.actor);
    let target_position = highest_role_position(&guild, target);
    if !hierarchy_allows(
        actor_position,
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

fn require_actor_permission(
    moderator: &Moderator<'_>,
    required: serenity::Permissions,
) -> Result<()> {
    let guild = moderator
        .discord
        .cache
        .guild(moderator.guild_id)
        .context("server is not available in the Discord cache")?;
    if !guild
        .member_permissions(&moderator.actor)
        .contains(required)
    {
        bail!(
            "invoker is missing {}",
            required.get_permission_names().join(", ")
        );
    }
    Ok(())
}

pub(super) fn require_actor_channel_permission(
    moderator: &Moderator<'_>,
    channel_id: serenity::ChannelId,
    required: serenity::Permissions,
) -> Result<()> {
    let access = crate::tools::discord::resolve_channel(
        moderator.discord,
        moderator.guild_id,
        &moderator.actor,
        channel_id,
    )?;
    if !access.permissions.contains(required) {
        bail!(
            "invoker is missing {} in this channel",
            required.get_permission_names().join(", ")
        );
    }
    Ok(())
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

pub(super) async fn finish(moderator: &Moderator<'_>, new_case: NewCase<'_>) -> Result<Outcome> {
    let dm_delivered = new_case.dm_delivered;
    let case = insert_case(
        moderator.db,
        moderator.guild_id,
        moderator.actor.user.id,
        &new_case,
    )
    .await?;
    post_log(moderator.discord, moderator.db, &case).await;
    Ok(Outcome { case, dm_delivered })
}
