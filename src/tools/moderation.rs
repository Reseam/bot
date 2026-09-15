use std::sync::Arc;

use anyhow::{Context, Result, bail};
use poise::serenity_prelude as serenity;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{Snowflake, Tool, ToolOutput};
use crate::chat::Run;
use crate::moderation::{Moderator, actions, cases_for};
use crate::text::parse_duration;

const MAX_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(28 * 24 * 60 * 60);
const MAX_BAN_DURATION: std::time::Duration = std::time::Duration::from_secs(365 * 24 * 60 * 60);

pub fn tools(run: &Arc<Run>) -> Vec<Tool> {
    vec![
        Tool::new(
            "moderation_warn",
            "Warn a server member when their conduct needs a recorded warning. This requires invoker approval.",
            run.clone(),
            warn,
        ),
        Tool::new(
            "moderation_timeout",
            "Timeout a server member for up to 28 days when they need temporary restriction. This requires invoker approval.",
            run.clone(),
            timeout,
        ),
        Tool::new(
            "moderation_kick",
            "Kick a server member when they must be removed without a ban. This requires invoker approval.",
            run.clone(),
            kick,
        ),
        Tool::new(
            "moderation_ban",
            "Ban a server member permanently or temporarily when they must be excluded. This requires invoker approval.",
            run.clone(),
            ban,
        ),
        Tool::new(
            "moderation_delete_message",
            "Delete one Discord message when specific content must be removed. This requires invoker approval.",
            run.clone(),
            delete_message,
        ),
        Tool::new(
            "moderation_purge",
            "Bulk-delete matching recent messages when a channel needs cleanup. This requires invoker approval.",
            run.clone(),
            purge,
        ),
        Tool::new(
            "moderation_cases",
            "List the newest moderation cases when reviewing a user's moderation history. The invoker must have Moderate Members.",
            run.clone(),
            cases,
        ),
    ]
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Warn {
    user_id: Snowflake,
    reason: String,
}

async fn warn(run: Arc<Run>, args: Warn) -> Result<ToolOutput> {
    let target = member(&run, args.user_id).await?;
    approve_member(&run, "moderation_warn", "Warn", &target, None, &args.reason).await?;
    let moderator = moderator(&run);
    let outcome = actions::warn(&moderator, &target, &args.reason).await?;
    Ok(outcome_text("Warned", target.user.id, &outcome))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Timeout {
    user_id: Snowflake,
    duration: String,
    reason: String,
}

async fn timeout(run: Arc<Run>, args: Timeout) -> Result<ToolOutput> {
    let duration = parse_duration(&args.duration)?;
    if duration > MAX_TIMEOUT {
        bail!("duration must not exceed 28 days");
    }
    let target = member(&run, args.user_id).await?;
    approve_member(
        &run,
        "moderation_timeout",
        "Timeout",
        &target,
        Some(&args.duration),
        &args.reason,
    )
    .await?;
    let moderator = moderator(&run);
    let outcome = actions::timeout(&moderator, &target, duration, &args.reason).await?;
    Ok(outcome_text("Timed out", target.user.id, &outcome))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct MemberAction {
    user_id: Snowflake,
    reason: String,
}

async fn kick(run: Arc<Run>, args: MemberAction) -> Result<ToolOutput> {
    let target = member(&run, args.user_id).await?;
    approve_member(&run, "moderation_kick", "Kick", &target, None, &args.reason).await?;
    let moderator = moderator(&run);
    let outcome = actions::kick(&moderator, &target, &args.reason).await?;
    Ok(outcome_text("Kicked", target.user.id, &outcome))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Ban {
    user_id: Snowflake,
    reason: String,
    duration: Option<String>,
    delete_message_days: Option<u8>,
}

async fn ban(run: Arc<Run>, args: Ban) -> Result<ToolOutput> {
    let duration = args.duration.as_deref().map(parse_duration).transpose()?;
    if duration.is_some_and(|duration| duration > MAX_BAN_DURATION) {
        bail!("duration must not exceed 365 days");
    }
    let delete_days = args.delete_message_days.unwrap_or(0);
    if delete_days > 7 {
        bail!("delete_message_days must be between 0 and 7");
    }
    let target = member(&run, args.user_id).await?;
    approve_member(
        &run,
        "moderation_ban",
        "Ban",
        &target,
        args.duration.as_deref(),
        &args.reason,
    )
    .await?;
    let moderator = moderator(&run);
    let outcome = actions::ban(&moderator, &target, &args.reason, duration, delete_days).await?;
    Ok(outcome_text("Banned", target.user.id, &outcome))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct DeleteMessage {
    channel_id: Snowflake,
    message_id: Snowflake,
    reason: String,
}

async fn delete_message(run: Arc<Run>, args: DeleteMessage) -> Result<ToolOutput> {
    let channel = serenity::ChannelId::new(args.channel_id.get());
    let message = serenity::MessageId::new(args.message_id.get());
    run.approve(
        "moderation_delete_message",
        &format!(
            "Delete message `{message}` in <#{channel}>\nReason: {}",
            args.reason
        ),
    )
    .await?;
    let moderator = moderator(&run);
    let outcome = actions::delete_message(&moderator, channel, message, &args.reason).await?;
    Ok(ToolOutput::text(format!(
        "Deleted message {message}. Case #{}.",
        outcome.case.id
    )))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Purge {
    channel_id: Option<Snowflake>,
    count: u8,
    user_id: Option<Snowflake>,
    contains: Option<String>,
    bots: Option<bool>,
}

async fn purge(run: Arc<Run>, args: Purge) -> Result<ToolOutput> {
    if !(1..=100).contains(&args.count) {
        bail!("count must be between 1 and 100");
    }
    let channel = args
        .channel_id
        .map_or(run.channel_id, |id| serenity::ChannelId::new(id.get()));
    let target = args.user_id.map(|id| serenity::UserId::new(id.get()));
    run.approve(
        "moderation_purge",
        &format!(
            "Delete up to {} messages in <#{}>\nUser: {}\nContains: {}\nBots: {}",
            args.count,
            channel,
            target.map_or_else(|| "any".to_owned(), |id| format!("<@{id}> (`{id}`)")),
            args.contains.as_deref().unwrap_or("any"),
            args.bots
                .map_or_else(|| "any".to_owned(), |value| value.to_string())
        ),
    )
    .await?;
    let moderator = moderator(&run);
    let deleted = actions::purge_messages(
        &moderator,
        channel,
        args.count,
        target,
        args.contains.as_deref(),
        args.bots,
    )
    .await?;
    let outcome = actions::record_purge(&moderator, target, channel, deleted).await?;
    Ok(ToolOutput::text(format!(
        "Deleted {deleted} messages. Case #{}.",
        outcome.case.id
    )))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Cases {
    user_id: Snowflake,
}

async fn cases(run: Arc<Run>, args: Cases) -> Result<ToolOutput> {
    let allowed = run
        .discord
        .cache
        .guild(run.guild_id)
        .context("server is not available in the Discord cache")?
        .member_permissions(&run.invoker)
        .contains(serenity::Permissions::MODERATE_MEMBERS);
    if !allowed {
        bail!("invoker is missing MODERATE_MEMBERS");
    }
    let target = serenity::UserId::new(args.user_id.get());
    let cases = cases_for(&run.app.db, run.guild_id, target).await?;
    let text = if cases.is_empty() {
        format!("No moderation cases for {target}.")
    } else {
        cases
            .iter()
            .map(|case| {
                format!(
                    "#{} {} at {} by {}: {}",
                    case.id, case.action, case.created_at, case.moderator_id, case.reason
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    Ok(ToolOutput::text(text))
}

async fn member(run: &Run, id: Snowflake) -> Result<serenity::Member> {
    run.guild_id
        .member(&run.discord, serenity::UserId::new(id.get()))
        .await
        .context("failed to fetch target member")
}

fn moderator(run: &Run) -> Moderator<'_> {
    Moderator {
        discord: &run.discord,
        db: &run.app.db,
        guild_id: run.guild_id,
        actor: run.invoker.clone(),
        via_ai: true,
    }
}

async fn approve_member(
    run: &Run,
    tool: &str,
    action: &str,
    target: &serenity::Member,
    duration: Option<&str>,
    reason: &str,
) -> Result<()> {
    run.approve(
        tool,
        &format!(
            "{action} <@{}> (`{}`)\nDuration: {}\nReason: {reason}",
            target.user.id,
            target.user.id,
            duration.unwrap_or("none")
        ),
    )
    .await
}

fn outcome_text(verb: &str, target: serenity::UserId, outcome: &actions::Outcome) -> ToolOutput {
    let dm = outcome.dm_delivered.map_or(String::new(), |delivered| {
        format!(" DM {}.", if delivered { "delivered" } else { "failed" })
    });
    ToolOutput::text(format!("{verb} {target}. Case #{}.{dm}", outcome.case.id))
}
