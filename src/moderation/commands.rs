use anyhow::{Context as _, Result, bail};
use poise::serenity_prelude as serenity;

use super::{Moderator, actions};
use crate::text::parse_duration;
use crate::{Data, Error};

const MAX_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(28 * 24 * 60 * 60);
const MAX_BAN_DURATION: std::time::Duration = std::time::Duration::from_secs(365 * 24 * 60 * 60);

type Command = poise::Command<Data, Error>;
type Context<'a> = poise::Context<'a, Data, Error>;

mod cases;

pub fn all() -> Vec<Command> {
    vec![
        warn(),
        note(),
        timeout(),
        untimeout(),
        kick(),
        ban(),
        unban(),
        purge(),
        slowmode(),
        lock(),
        unlock(),
    ]
    .into_iter()
    .chain(cases::all())
    .collect()
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MODERATE_MEMBERS",
    required_permissions = "MODERATE_MEMBERS",
    required_bot_permissions = "MODERATE_MEMBERS"
)]
async fn warn(ctx: Context<'_>, user: serenity::Member, reason: String) -> Result<()> {
    let moderator = moderator(ctx, false).await?;
    let outcome = actions::warn(&moderator, &user, &reason).await?;
    action_reply(ctx, "Warned", user.user.id, &outcome).await
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MODERATE_MEMBERS",
    required_permissions = "MODERATE_MEMBERS"
)]
async fn note(ctx: Context<'_>, user: serenity::Member, text: String) -> Result<()> {
    let moderator = moderator(ctx, false).await?;
    let outcome = actions::note(&moderator, &user, &text).await?;
    action_reply(ctx, "Added a note for", user.user.id, &outcome).await
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MODERATE_MEMBERS",
    required_permissions = "MODERATE_MEMBERS",
    required_bot_permissions = "MODERATE_MEMBERS"
)]
async fn timeout(
    ctx: Context<'_>,
    user: serenity::Member,
    #[description = "Examples: 30m, 2h, 7d"] duration: String,
    reason: Option<String>,
) -> Result<()> {
    let duration = parse_duration(&duration)?;
    if duration > MAX_TIMEOUT {
        bail!("duration must not exceed 28 days");
    }
    let moderator = moderator(ctx, false).await?;
    let outcome = actions::timeout(
        &moderator,
        &user,
        duration,
        reason.as_deref().unwrap_or("No reason provided"),
    )
    .await?;
    action_reply(ctx, "Timed out", user.user.id, &outcome).await
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MODERATE_MEMBERS",
    required_permissions = "MODERATE_MEMBERS",
    required_bot_permissions = "MODERATE_MEMBERS"
)]
async fn untimeout(ctx: Context<'_>, user: serenity::Member, reason: Option<String>) -> Result<()> {
    let moderator = moderator(ctx, false).await?;
    let outcome = actions::untimeout(
        &moderator,
        &user,
        reason.as_deref().unwrap_or("No reason provided"),
    )
    .await?;
    action_reply(ctx, "Removed timeout from", user.user.id, &outcome).await
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "KICK_MEMBERS",
    required_permissions = "KICK_MEMBERS",
    required_bot_permissions = "KICK_MEMBERS"
)]
async fn kick(ctx: Context<'_>, user: serenity::Member, reason: Option<String>) -> Result<()> {
    let moderator = moderator(ctx, false).await?;
    let outcome = actions::kick(
        &moderator,
        &user,
        reason.as_deref().unwrap_or("No reason provided"),
    )
    .await?;
    action_reply(ctx, "Kicked", user.user.id, &outcome).await
}

#[derive(Clone, Copy, Debug, poise::ChoiceParameter)]
enum DeleteMessages {
    #[name = "None"]
    None,
    #[name = "1 day"]
    Day,
    #[name = "7 days"]
    Week,
}

impl DeleteMessages {
    const fn days(self) -> u8 {
        match self {
            Self::None => 0,
            Self::Day => 1,
            Self::Week => 7,
        }
    }
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "BAN_MEMBERS",
    required_permissions = "BAN_MEMBERS",
    required_bot_permissions = "BAN_MEMBERS"
)]
async fn ban(
    ctx: Context<'_>,
    user: serenity::Member,
    reason: Option<String>,
    #[description = "Optional temporary-ban duration"] duration: Option<String>,
    delete_messages: Option<DeleteMessages>,
) -> Result<()> {
    let duration = duration.as_deref().map(parse_duration).transpose()?;
    if duration.is_some_and(|duration| duration > MAX_BAN_DURATION) {
        bail!("duration must not exceed 365 days");
    }
    let moderator = moderator(ctx, false).await?;
    let outcome = actions::ban(
        &moderator,
        &user,
        reason.as_deref().unwrap_or("No reason provided"),
        duration,
        delete_messages.unwrap_or(DeleteMessages::None).days(),
    )
    .await?;
    action_reply(ctx, "Banned", user.user.id, &outcome).await
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "BAN_MEMBERS",
    required_permissions = "BAN_MEMBERS",
    required_bot_permissions = "BAN_MEMBERS"
)]
async fn unban(ctx: Context<'_>, user_id: String, reason: Option<String>) -> Result<()> {
    let target = parse_user_id(&user_id)?;
    let moderator = moderator(ctx, false).await?;
    let outcome = actions::unban(
        &moderator,
        target,
        reason.as_deref().unwrap_or("No reason provided"),
    )
    .await?;
    action_reply(ctx, "Unbanned", target, &outcome).await
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_MESSAGES",
    required_permissions = "MANAGE_MESSAGES",
    required_bot_permissions = "MANAGE_MESSAGES | READ_MESSAGE_HISTORY"
)]
async fn purge(
    ctx: Context<'_>,
    #[min = 1]
    #[max = 100]
    count: u8,
    user: Option<serenity::User>,
    contains: Option<String>,
    bots: Option<bool>,
) -> Result<()> {
    let moderator = moderator(ctx, false).await?;
    let deleted = actions::purge_messages(
        &moderator,
        ctx.channel_id(),
        count,
        user.as_ref().map(|user| user.id),
        contains.as_deref(),
        bots,
    )
    .await?;
    actions::record_purge(
        &moderator,
        user.map(|user| user.id),
        ctx.channel_id(),
        deleted,
    )
    .await?;
    ctx.say(format!("Deleted {deleted} messages."))
        .await
        .context("failed to send purge result")?;
    Ok(())
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_CHANNELS",
    required_permissions = "MANAGE_CHANNELS",
    required_bot_permissions = "MANAGE_CHANNELS"
)]
async fn slowmode(
    ctx: Context<'_>,
    #[min = 0]
    #[max = 21600]
    seconds: u16,
    channel: Option<serenity::GuildChannel>,
) -> Result<()> {
    let moderator = moderator(ctx, false).await?;
    let channel = selected_channel(ctx, channel)?;
    let outcome = actions::set_slowmode(&moderator, &channel, seconds).await?;
    ctx.say(format!(
        "Set slowmode in <#{}> to {seconds} seconds. Case #{}.",
        channel.id, outcome.case.id
    ))
    .await
    .context("failed to send slowmode result")?;
    Ok(())
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_CHANNELS",
    required_permissions = "MANAGE_CHANNELS",
    required_bot_permissions = "MANAGE_CHANNELS"
)]
async fn lock(
    ctx: Context<'_>,
    channel: Option<serenity::GuildChannel>,
    reason: Option<String>,
) -> Result<()> {
    lock_command(ctx, channel, reason, true).await
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_CHANNELS",
    required_permissions = "MANAGE_CHANNELS",
    required_bot_permissions = "MANAGE_CHANNELS"
)]
async fn unlock(
    ctx: Context<'_>,
    channel: Option<serenity::GuildChannel>,
    reason: Option<String>,
) -> Result<()> {
    lock_command(ctx, channel, reason, false).await
}

async fn lock_command(
    ctx: Context<'_>,
    channel: Option<serenity::GuildChannel>,
    reason: Option<String>,
    locked: bool,
) -> Result<()> {
    let moderator = moderator(ctx, false).await?;
    let channel = selected_channel(ctx, channel)?;
    let outcome = actions::set_locked(
        &moderator,
        &channel,
        locked,
        reason.as_deref().unwrap_or("No reason provided"),
    )
    .await?;
    ctx.say(format!(
        "{} <#{}>. Case #{}.",
        if locked { "Locked" } else { "Unlocked" },
        channel.id,
        outcome.case.id
    ))
    .await
    .context("failed to send channel lock result")?;
    Ok(())
}

async fn moderator(ctx: Context<'_>, via_ai: bool) -> Result<Moderator<'_>> {
    let actor = ctx
        .author_member()
        .await
        .context("failed to fetch command member")?;
    Ok(Moderator {
        discord: ctx.serenity_context(),
        db: &ctx.data().db,
        guild_id: ctx.guild_id().context("moderation command has no guild")?,
        actor: actor.into_owned(),
        via_ai,
    })
}

fn selected_channel(
    ctx: Context<'_>,
    channel: Option<serenity::GuildChannel>,
) -> Result<serenity::GuildChannel> {
    channel
        .or_else(|| {
            ctx.guild()
                .and_then(|guild| guild.channels.get(&ctx.channel_id()).cloned())
        })
        .context("current channel is not available in the Discord cache")
}

async fn action_reply(
    ctx: Context<'_>,
    verb: &str,
    target: serenity::UserId,
    outcome: &actions::Outcome,
) -> Result<()> {
    let dm = outcome.dm_delivered.map_or(String::new(), |delivered| {
        format!(" DM {}.", if delivered { "delivered" } else { "failed" })
    });
    ctx.say(format!(
        "{verb} <@{target}>. Case #{}.{dm}",
        outcome.case.id
    ))
    .await
    .context("failed to send moderation result")?;
    Ok(())
}

fn parse_user_id(input: &str) -> Result<serenity::UserId> {
    let id = input
        .parse::<u64>()
        .context("user_id must be an unsigned integer")?;
    if id == 0 {
        bail!("user_id must not be zero");
    }
    Ok(serenity::UserId::new(id))
}
