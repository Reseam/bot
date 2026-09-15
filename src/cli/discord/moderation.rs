use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Subcommand;
use poise::serenity_prelude as serenity;

use super::channel;
use crate::chat::Run;
use crate::cli::{CommandOutput, snowflake};
use crate::moderation::commands::{parse_ban_duration, parse_timeout};
use crate::moderation::{Moderator, actions};

#[derive(Subcommand)]
pub enum Mod {
    /// Warn a member and send them a DM
    Warn {
        #[arg(value_parser = snowflake)]
        user: u64,
        #[arg(long)]
        reason: String,
    },
    /// Timeout a member for up to 28 days
    Timeout {
        #[arg(value_parser = snowflake)]
        user: u64,
        /// Such as 30m, 2h, or 7d
        duration: String,
        #[arg(long)]
        reason: String,
    },
    /// Remove a member's timeout
    Untimeout {
        #[arg(value_parser = snowflake)]
        user: u64,
        #[arg(long)]
        reason: String,
    },
    /// Kick a member
    Kick {
        #[arg(value_parser = snowflake)]
        user: u64,
        #[arg(long)]
        reason: String,
    },
    /// Ban a user, including people who already left, optionally for a limited time
    Ban {
        #[arg(value_parser = snowflake)]
        user: u64,
        #[arg(long)]
        reason: String,
        /// Temporary ban length, such as 3d, up to 365 days
        #[arg(long)]
        duration: Option<String>,
        /// Delete their messages from the last 0 to 7 days
        #[arg(long, default_value_t = 0, value_parser = clap::value_parser!(u8).range(0..=7))]
        delete_days: u8,
    },
    /// Unban a user
    Unban {
        #[arg(value_parser = snowflake)]
        user: u64,
        #[arg(long)]
        reason: String,
    },
    /// Delete one message
    Delete {
        #[arg(value_parser = snowflake)]
        message: u64,
        #[arg(long, value_parser = snowflake)]
        channel: Option<u64>,
        #[arg(long)]
        reason: String,
    },
    /// Delete up to 100 matching messages from the last 14 days
    Purge {
        #[arg(value_parser = clap::value_parser!(u8).range(1..=100))]
        count: u8,
        #[arg(long, value_parser = snowflake)]
        channel: Option<u64>,
        /// Only messages from this user
        #[arg(long, value_parser = snowflake)]
        user: Option<u64>,
        /// Only messages containing this text
        #[arg(long)]
        contains: Option<String>,
        /// Only bot messages (true) or only non-bot messages (false)
        #[arg(long)]
        bots: Option<bool>,
    },
    /// Set slowmode in seconds, 0 to 21600
    Slowmode {
        #[arg(value_parser = clap::value_parser!(u16).range(0..=21_600))]
        seconds: u16,
        #[arg(long, value_parser = snowflake)]
        channel: Option<u64>,
    },
    /// Stop regular members from sending messages in a channel
    Lock {
        #[arg(long, value_parser = snowflake)]
        channel: Option<u64>,
        #[arg(long)]
        reason: String,
    },
    /// Restore the permissions a channel had before it was locked
    Unlock {
        #[arg(long, value_parser = snowflake)]
        channel: Option<u64>,
        #[arg(long)]
        reason: String,
    },
}

pub async fn run(run: &Arc<Run>, command: Mod) -> Result<CommandOutput> {
    let moderator = Moderator {
        discord: &run.discord,
        db: &run.app.db,
        guild_id: run.guild_id,
        actor: run.invoker.clone(),
        via_ai: true,
    };
    let text = match command {
        Mod::Warn { user, reason } => {
            let target = required_member(&moderator, user).await?;
            check(&moderator, &target, serenity::Permissions::MODERATE_MEMBERS)?;
            approve(
                run,
                "warn",
                user,
                &format!("Warn <@{user}>\nReason: {reason}"),
            )
            .await?;
            actions::warn(&moderator, &target, &reason)
                .await?
                .describe("Warned", target.user.id)
        }
        Mod::Timeout {
            user,
            duration,
            reason,
        } => {
            let length = parse_timeout(&duration)?;
            let target = required_member(&moderator, user).await?;
            check(&moderator, &target, serenity::Permissions::MODERATE_MEMBERS)?;
            approve(
                run,
                "timeout",
                user,
                &format!("Timeout <@{user}> for {duration}\nReason: {reason}"),
            )
            .await?;
            actions::timeout(&moderator, &target, length, &reason)
                .await?
                .describe("Timed out", target.user.id)
        }
        Mod::Untimeout { user, reason } => {
            let target = required_member(&moderator, user).await?;
            check(&moderator, &target, serenity::Permissions::MODERATE_MEMBERS)?;
            approve(
                run,
                "untimeout",
                user,
                &format!("Remove the timeout from <@{user}>\nReason: {reason}"),
            )
            .await?;
            actions::untimeout(&moderator, &target, &reason)
                .await?
                .describe("Removed the timeout from", target.user.id)
        }
        Mod::Kick { user, reason } => {
            let target = required_member(&moderator, user).await?;
            check(&moderator, &target, serenity::Permissions::KICK_MEMBERS)?;
            approve(
                run,
                "kick",
                user,
                &format!("Kick <@{user}>\nReason: {reason}"),
            )
            .await?;
            actions::kick(&moderator, &target, &reason)
                .await?
                .describe("Kicked", target.user.id)
        }
        Mod::Ban {
            user,
            reason,
            duration,
            delete_days,
        } => {
            let length = duration.as_deref().map(parse_ban_duration).transpose()?;
            let user_id = serenity::UserId::new(user);
            let target = actions::member(&moderator, user_id).await?;
            actions::validate_target(
                &moderator,
                user_id,
                target.as_ref(),
                serenity::Permissions::BAN_MEMBERS,
            )?;
            approve(
                run,
                "ban",
                user,
                &format!(
                    "Ban <@{user}> {}\nDelete messages from the last {delete_days} days\nReason: {reason}",
                    duration.map_or_else(|| "permanently".to_owned(), |value| format!("for {value}"))
                ),
            )
            .await?;
            actions::ban(
                &moderator,
                user_id,
                target.as_ref(),
                &reason,
                length,
                delete_days,
            )
            .await?
            .describe("Banned", user_id)
        }
        Mod::Unban { user, reason } => {
            let user_id = serenity::UserId::new(user);
            actions::validate_target(
                &moderator,
                user_id,
                None,
                serenity::Permissions::BAN_MEMBERS,
            )?;
            approve(
                run,
                "unban",
                user,
                &format!("Unban <@{user}>\nReason: {reason}"),
            )
            .await?;
            actions::unban(&moderator, user_id, &reason)
                .await?
                .describe("Unbanned", user_id)
        }
        Mod::Delete {
            message,
            channel: requested,
            reason,
        } => {
            let channel_id = channel(run, requested);
            actions::require_channel_permission(
                &moderator,
                channel_id,
                serenity::Permissions::MANAGE_MESSAGES,
            )
            .await?;
            approve(
                run,
                "delete",
                message,
                &format!("Delete message `{message}` in <#{channel_id}>\nReason: {reason}"),
            )
            .await?;
            actions::delete_message(
                &moderator,
                channel_id,
                serenity::MessageId::new(message),
                &reason,
            )
            .await?;
            format!("Deleted message {message}.")
        }
        Mod::Purge {
            count,
            channel: requested,
            user,
            contains,
            bots,
        } => {
            let channel_id = channel(run, requested);
            actions::require_channel_permission(
                &moderator,
                channel_id,
                serenity::Permissions::MANAGE_MESSAGES,
            )
            .await?;
            approve(
                run,
                "purge",
                channel_id.get(),
                &format!(
                    "Delete up to {count} messages in <#{channel_id}>\nUser: {}\nContains: {}\nBots: {}",
                    user.map_or_else(|| "any".to_owned(), |id| format!("<@{id}>")),
                    contains.as_deref().unwrap_or("any"),
                    bots.map_or_else(|| "any".to_owned(), |value| value.to_string())
                ),
            )
            .await?;
            let deleted = actions::purge(
                &moderator,
                channel_id,
                usize::from(count),
                user.map(serenity::UserId::new),
                contains.as_deref(),
                bots,
            )
            .await?;
            format!("Deleted {deleted} messages.")
        }
        Mod::Slowmode {
            seconds,
            channel: requested,
        } => {
            let channel_id = channel(run, requested);
            actions::require_channel_permission(
                &moderator,
                channel_id,
                serenity::Permissions::MANAGE_CHANNELS,
            )
            .await?;
            approve(
                run,
                "slowmode",
                channel_id.get(),
                &format!("Set slowmode in <#{channel_id}> to {seconds} seconds"),
            )
            .await?;
            actions::set_slowmode(&moderator, channel_id, seconds).await?;
            format!("Set slowmode in <#{channel_id}> to {seconds} seconds.")
        }
        Mod::Lock {
            channel: requested,
            reason,
        } => {
            let channel_id = channel(run, requested);
            actions::require_channel_permission(
                &moderator,
                channel_id,
                serenity::Permissions::MANAGE_CHANNELS,
            )
            .await?;
            approve(
                run,
                "lock",
                channel_id.get(),
                &format!("Lock <#{channel_id}>\nReason: {reason}"),
            )
            .await?;
            actions::lock(&moderator, channel_id, &reason).await?;
            format!("Locked <#{channel_id}>.")
        }
        Mod::Unlock {
            channel: requested,
            reason,
        } => {
            let channel_id = channel(run, requested);
            actions::require_channel_permission(
                &moderator,
                channel_id,
                serenity::Permissions::MANAGE_CHANNELS,
            )
            .await?;
            approve(
                run,
                "unlock",
                channel_id.get(),
                &format!("Unlock <#{channel_id}>\nReason: {reason}"),
            )
            .await?;
            actions::unlock(&moderator, channel_id, &reason).await?;
            format!("Unlocked <#{channel_id}>.")
        }
    };
    Ok(CommandOutput::text(text))
}

async fn required_member(moderator: &Moderator<'_>, user: u64) -> Result<serenity::Member> {
    actions::member(moderator, serenity::UserId::new(user))
        .await?
        .with_context(|| format!("user {user} is not a member of this server"))
}

fn check(
    moderator: &Moderator<'_>,
    target: &serenity::Member,
    required: serenity::Permissions,
) -> Result<()> {
    actions::validate_target(moderator, target.user.id, Some(target), required)
}

async fn approve(run: &Run, action: &str, target: u64, preview: &str) -> Result<()> {
    run.approve(&format!("discord mod {action} {target}"), preview)
        .await
}
