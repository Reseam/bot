use anyhow::{Context as _, Result, bail};
use poise::serenity_prelude as serenity;

use super::{Moderator, actions};
use crate::settings;
use crate::text::parse_duration;
use crate::{Data, Error};

pub const MAX_TIMEOUT: std::time::Duration = std::time::Duration::from_secs(28 * 24 * 60 * 60);
pub const MAX_BAN_DURATION: std::time::Duration =
    std::time::Duration::from_secs(365 * 24 * 60 * 60);
const NO_REASON: &str = "No reason provided";

type Command = poise::Command<Data, Error>;
type Context<'a> = poise::Context<'a, Data, Error>;

pub fn all() -> Vec<Command> {
    vec![
        warn(),
        timeout(),
        untimeout(),
        kick(),
        ban(),
        unban(),
        purge(),
        slowmode(),
        lock(),
        unlock(),
        modlog(),
    ]
}

pub fn parse_timeout(input: &str) -> Result<std::time::Duration> {
    let duration = parse_duration(input)?;
    if duration > MAX_TIMEOUT {
        bail!("timeout must not exceed 28 days");
    }
    Ok(duration)
}

pub fn parse_ban_duration(input: &str) -> Result<std::time::Duration> {
    let duration = parse_duration(input)?;
    if duration > MAX_BAN_DURATION {
        bail!("ban duration must not exceed 365 days");
    }
    Ok(duration)
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MODERATE_MEMBERS",
    required_bot_permissions = "MODERATE_MEMBERS"
)]
async fn warn(ctx: Context<'_>, user: serenity::Member, reason: String) -> Result<()> {
    let outcome = actions::warn(&moderator(ctx).await?, &user, &reason).await?;
    say(ctx, outcome.describe("Warned", user.user.id)).await
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MODERATE_MEMBERS",
    required_bot_permissions = "MODERATE_MEMBERS"
)]
async fn timeout(
    ctx: Context<'_>,
    user: serenity::Member,
    #[description = "Examples: 30m, 2h, 7d"] duration: String,
    reason: Option<String>,
) -> Result<()> {
    let duration = parse_timeout(&duration)?;
    let outcome = actions::timeout(
        &moderator(ctx).await?,
        &user,
        duration,
        reason.as_deref().unwrap_or(NO_REASON),
    )
    .await?;
    say(ctx, outcome.describe("Timed out", user.user.id)).await
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MODERATE_MEMBERS",
    required_bot_permissions = "MODERATE_MEMBERS"
)]
async fn untimeout(ctx: Context<'_>, user: serenity::Member, reason: Option<String>) -> Result<()> {
    let outcome = actions::untimeout(
        &moderator(ctx).await?,
        &user,
        reason.as_deref().unwrap_or(NO_REASON),
    )
    .await?;
    say(
        ctx,
        outcome.describe("Removed the timeout from", user.user.id),
    )
    .await
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "KICK_MEMBERS",
    required_bot_permissions = "KICK_MEMBERS"
)]
async fn kick(ctx: Context<'_>, user: serenity::Member, reason: Option<String>) -> Result<()> {
    let outcome = actions::kick(
        &moderator(ctx).await?,
        &user,
        reason.as_deref().unwrap_or(NO_REASON),
    )
    .await?;
    say(ctx, outcome.describe("Kicked", user.user.id)).await
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
    required_bot_permissions = "BAN_MEMBERS"
)]
async fn ban(
    ctx: Context<'_>,
    #[description = "A member or any user, including people who already left"] user: serenity::User,
    reason: Option<String>,
    #[description = "Temporary ban length, such as 3d"] duration: Option<String>,
    delete_messages: Option<DeleteMessages>,
) -> Result<()> {
    let duration = duration.as_deref().map(parse_ban_duration).transpose()?;
    let moderator = moderator(ctx).await?;
    let member = actions::member(&moderator, user.id).await?;
    let outcome = actions::ban(
        &moderator,
        user.id,
        member.as_ref(),
        reason.as_deref().unwrap_or(NO_REASON),
        duration,
        delete_messages.unwrap_or(DeleteMessages::None).days(),
    )
    .await?;
    say(ctx, outcome.describe("Banned", user.id)).await
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "BAN_MEMBERS",
    required_bot_permissions = "BAN_MEMBERS"
)]
async fn unban(ctx: Context<'_>, user: serenity::User, reason: Option<String>) -> Result<()> {
    let outcome = actions::unban(
        &moderator(ctx).await?,
        user.id,
        reason.as_deref().unwrap_or(NO_REASON),
    )
    .await?;
    say(ctx, outcome.describe("Unbanned", user.id)).await
}

#[poise::command(
    slash_command,
    guild_only,
    ephemeral,
    default_member_permissions = "MANAGE_MESSAGES",
    required_bot_permissions = "MANAGE_MESSAGES | READ_MESSAGE_HISTORY"
)]
async fn purge(
    ctx: Context<'_>,
    #[min = 1]
    #[max = 100]
    count: u8,
    #[description = "Only messages from this user"] user: Option<serenity::User>,
    #[description = "Only messages containing this text"] contains: Option<String>,
    #[description = "Only bot messages, or only non-bot messages"] bots: Option<bool>,
) -> Result<()> {
    ctx.defer_ephemeral()
        .await
        .context("failed to defer purge")?;
    let deleted = actions::purge(
        &moderator(ctx).await?,
        ctx.channel_id(),
        usize::from(count),
        user.map(|user| user.id),
        contains.as_deref(),
        bots,
    )
    .await?;
    say(ctx, format!("Deleted {deleted} messages.")).await
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_CHANNELS",
    required_bot_permissions = "MANAGE_CHANNELS"
)]
async fn slowmode(
    ctx: Context<'_>,
    #[min = 0]
    #[max = 21600]
    seconds: u16,
    #[channel_types("Text", "News", "PublicThread", "PrivateThread", "NewsThread")] channel: Option<
        serenity::GuildChannel,
    >,
) -> Result<()> {
    let channel_id = channel.map_or(ctx.channel_id(), |channel| channel.id);
    actions::set_slowmode(&moderator(ctx).await?, channel_id, seconds).await?;
    say(
        ctx,
        format!("Set slowmode in <#{channel_id}> to {seconds} seconds."),
    )
    .await
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_CHANNELS",
    required_bot_permissions = "MANAGE_ROLES"
)]
async fn lock(
    ctx: Context<'_>,
    #[channel_types("Text", "News")] channel: Option<serenity::GuildChannel>,
    reason: Option<String>,
) -> Result<()> {
    let channel_id = channel.map_or(ctx.channel_id(), |channel| channel.id);
    actions::lock(
        &moderator(ctx).await?,
        channel_id,
        reason.as_deref().unwrap_or(NO_REASON),
    )
    .await?;
    say(ctx, format!("Locked <#{channel_id}>.")).await
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_CHANNELS",
    required_bot_permissions = "MANAGE_ROLES"
)]
async fn unlock(
    ctx: Context<'_>,
    #[channel_types("Text", "News")] channel: Option<serenity::GuildChannel>,
    reason: Option<String>,
) -> Result<()> {
    let channel_id = channel.map_or(ctx.channel_id(), |channel| channel.id);
    actions::unlock(
        &moderator(ctx).await?,
        channel_id,
        reason.as_deref().unwrap_or(NO_REASON),
    )
    .await?;
    say(ctx, format!("Unlocked <#{channel_id}>.")).await
}

#[poise::command(
    slash_command,
    guild_only,
    ephemeral,
    default_member_permissions = "MANAGE_GUILD"
)]
async fn modlog(
    ctx: Context<'_>,
    #[description = "Channel for moderation logs"]
    #[channel_types("Text")]
    channel: Option<serenity::GuildChannel>,
) -> Result<()> {
    let guild_id = ctx.guild_id().context("modlog command has no guild")?;
    let db = &ctx.data().db;
    let text = match channel {
        Some(channel) => {
            settings::set_mod_log_channel(db, guild_id, channel.id).await?;
            format!("Moderation log set to <#{}>.", channel.id)
        }
        None => settings::mod_log_channel(db, guild_id).await?.map_or_else(
            || "No moderation log channel is configured.".to_owned(),
            |channel| format!("Moderation log: <#{channel}>."),
        ),
    };
    say(ctx, text).await
}

async fn moderator(ctx: Context<'_>) -> Result<Moderator<'_>> {
    Ok(Moderator {
        discord: ctx.serenity_context(),
        db: &ctx.data().db,
        guild_id: ctx.guild_id().context("moderation command has no guild")?,
        actor: ctx
            .author_member()
            .await
            .context("failed to fetch command member")?
            .into_owned(),
        via_ai: false,
    })
}

async fn say(ctx: Context<'_>, text: String) -> Result<()> {
    ctx.send(
        poise::CreateReply::new()
            .content(text)
            .allowed_mentions(serenity::CreateAllowedMentions::new()),
    )
    .await
    .context("failed to send moderation result")?;
    Ok(())
}
