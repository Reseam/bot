use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::Args;
use poise::serenity_prelude as serenity;

use super::channel;
use crate::chat::Run;
use crate::cli::{CommandOutput, snowflake};
use crate::discord::{
    channel_link, jump_link, require_permissions, resolve_channel, send_permission,
};
use crate::text::{DISCORD_MESSAGE_LIMIT, truncate_chars};

const PIN_MESSAGES: serenity::Permissions = serenity::Permissions::from_bits_retain(1 << 51);
const APPROVAL_PREVIEW_LIMIT: usize = 1_500;

#[derive(Args)]
pub struct Send {
    /// Channel or thread ID (default: the current channel)
    #[arg(long, value_parser = snowflake)]
    channel: Option<u64>,
    /// Reply to this message ID
    #[arg(long, value_parser = snowflake)]
    reply: Option<u64>,
    /// Message text. Read from stdin when omitted
    text: Option<String>,
}

#[derive(Args)]
pub struct React {
    #[arg(value_parser = snowflake)]
    message: u64,
    /// Unicode emoji, <:name:id>, or name:id
    emoji: String,
    #[arg(long, value_parser = snowflake)]
    channel: Option<u64>,
}

#[derive(Args)]
pub struct Thread {
    name: String,
    /// Start the thread from this message ID
    #[arg(long, value_parser = snowflake)]
    message: Option<u64>,
    #[arg(long, value_parser = snowflake)]
    channel: Option<u64>,
    /// Auto-archive after 60, 1440, 4320, or 10080 minutes
    #[arg(long, default_value_t = 1_440)]
    archive_minutes: u16,
}

#[derive(Args)]
pub struct Pin {
    #[arg(value_parser = snowflake)]
    message: u64,
    #[arg(long, value_parser = snowflake)]
    channel: Option<u64>,
}

pub async fn send(run: &Arc<Run>, args: Send, stdin: String) -> Result<CommandOutput> {
    let text = args.text.unwrap_or(stdin);
    let content = text.trim_end_matches('\n');
    if !(1..=DISCORD_MESSAGE_LIMIT).contains(&content.chars().count()) {
        bail!("message must be between 1 and {DISCORD_MESSAGE_LIMIT} characters");
    }
    let channel_id = channel(run, args.channel);
    let access = resolve_channel(&run.discord, run.guild_id, &run.invoker, channel_id).await?;
    let required = send_permission(&access.channel) | serenity::Permissions::VIEW_CHANNEL;
    if !access.permissions.contains(required) {
        bail!("invoker cannot send messages in #{}", access.channel.name);
    }
    if channel_id != run.channel_id {
        run.approve(
            &format!("discord send {channel_id}"),
            &format!(
                "Send to <#{channel_id}>:\n>>> {}",
                truncate_chars(content, APPROVAL_PREVIEW_LIMIT)
            ),
        )
        .await?;
    }
    let mut builder = serenity::CreateMessage::new()
        .content(content)
        .allowed_mentions(serenity::CreateAllowedMentions::new());
    if let Some(reply) = args.reply {
        builder = builder.reference_message((channel_id, serenity::MessageId::new(reply)));
    }
    let message = channel_id
        .send_message(&run.discord, builder)
        .await
        .context("failed to send Discord message")?;
    Ok(CommandOutput::text(format!(
        "Sent {}",
        jump_link(run.guild_id, channel_id, message.id)
    )))
}

pub async fn react(run: &Arc<Run>, args: React) -> Result<CommandOutput> {
    let channel_id = channel(run, args.channel);
    require_permissions(
        &run.discord,
        run.guild_id,
        &run.invoker,
        channel_id,
        serenity::Permissions::VIEW_CHANNEL | serenity::Permissions::ADD_REACTIONS,
    )
    .await?;
    let message_id = serenity::MessageId::new(args.message);
    channel_id
        .create_reaction(&run.discord, message_id, parse_emoji(&args.emoji)?)
        .await
        .context("failed to add Discord reaction")?;
    Ok(CommandOutput::text(format!(
        "Reacted to {}",
        jump_link(run.guild_id, channel_id, message_id)
    )))
}

pub async fn thread(run: &Arc<Run>, args: Thread) -> Result<CommandOutput> {
    if !(2..=100).contains(&args.name.chars().count()) {
        bail!("thread name must be between 2 and 100 characters");
    }
    let archive = match args.archive_minutes {
        60 => serenity::AutoArchiveDuration::OneHour,
        1_440 => serenity::AutoArchiveDuration::OneDay,
        4_320 => serenity::AutoArchiveDuration::ThreeDays,
        10_080 => serenity::AutoArchiveDuration::OneWeek,
        _ => bail!("--archive-minutes must be 60, 1440, 4320, or 10080"),
    };
    let channel_id = channel(run, args.channel);
    require_permissions(
        &run.discord,
        run.guild_id,
        &run.invoker,
        channel_id,
        serenity::Permissions::VIEW_CHANNEL | serenity::Permissions::CREATE_PUBLIC_THREADS,
    )
    .await?;
    let builder = serenity::CreateThread::new(args.name)
        .auto_archive_duration(archive)
        .kind(serenity::ChannelType::PublicThread);
    let thread = match args.message {
        Some(message) => {
            channel_id
                .create_thread_from_message(
                    &run.discord,
                    serenity::MessageId::new(message),
                    builder,
                )
                .await
        }
        None => channel_id.create_thread(&run.discord, builder).await,
    }
    .context("failed to create Discord thread")?;
    Ok(CommandOutput::text(format!(
        "Created thread {} {}",
        thread.id,
        channel_link(run.guild_id, thread.id)
    )))
}

pub async fn pin(run: &Arc<Run>, args: Pin, pinned: bool) -> Result<CommandOutput> {
    let channel_id = channel(run, args.channel);
    let access = resolve_channel(&run.discord, run.guild_id, &run.invoker, channel_id).await?;
    if !access
        .permissions
        .intersects(PIN_MESSAGES | serenity::Permissions::MANAGE_MESSAGES)
    {
        bail!("invoker is missing PIN_MESSAGES or MANAGE_MESSAGES in this channel");
    }
    let message_id = serenity::MessageId::new(args.message);
    if pinned {
        channel_id.pin(&run.discord, message_id).await
    } else {
        channel_id.unpin(&run.discord, message_id).await
    }
    .context("failed to change the message pin")?;
    Ok(CommandOutput::text(format!(
        "{} {}",
        if pinned { "Pinned" } else { "Unpinned" },
        jump_link(run.guild_id, channel_id, message_id)
    )))
}

fn parse_emoji(input: &str) -> Result<serenity::ReactionType> {
    let normalized = if !input.starts_with('<')
        && let Some((name, id)) = input.rsplit_once(':')
        && !name.is_empty()
        && id.parse::<u64>().is_ok()
    {
        format!("<:{name}:{id}>")
    } else {
        input.to_owned()
    };
    serenity::ReactionType::try_from(normalized.as_str())
        .map_err(|_| anyhow::anyhow!("emoji must be Unicode, <:name:id>, <a:name:id>, or name:id"))
}
