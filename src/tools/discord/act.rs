use std::sync::Arc;

use anyhow::{Context, Result, bail};
use poise::serenity_prelude as serenity;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{channel_id, channel_link, is_thread, jump_link, require_permissions, resolve_channel};
use crate::chat::Run;
use crate::text::truncate_chars;
use crate::tools::{Snowflake, Tool, ToolOutput};

const DEFAULT_ARCHIVE_MINUTES: u16 = 1_440;
const MESSAGE_LIMIT: usize = 2_000;
const PIN_MESSAGES: serenity::Permissions = serenity::Permissions::from_bits_retain(1 << 51);

pub fn tools(run: &Arc<Run>) -> Vec<Tool> {
    vec![
        Tool::new(
            "discord_send_message",
            "Send a Discord message when the user asks you to post or reply. Sending outside the current channel requires approval.",
            run.clone(),
            send_message,
        ),
        Tool::new(
            "discord_add_reaction",
            "Add a Unicode or custom emoji reaction when the user wants to acknowledge or mark a Discord message.",
            run.clone(),
            add_reaction,
        ),
        Tool::new(
            "discord_create_thread",
            "Create a public thread, optionally attached to a message, when a discussion needs its own space.",
            run.clone(),
            create_thread,
        ),
        Tool::new(
            "discord_pin_message",
            "Pin or unpin a Discord message when important information should be preserved or removed from the channel pins.",
            run.clone(),
            pin_message,
        ),
    ]
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SendMessage {
    channel_id: Snowflake,
    content: String,
    reply_to_message_id: Option<Snowflake>,
}

async fn send_message(run: Arc<Run>, args: SendMessage) -> Result<ToolOutput> {
    let length = args.content.chars().count();
    if !(1..=MESSAGE_LIMIT).contains(&length) {
        bail!("content must be between 1 and 2000 characters");
    }
    let channel_id = serenity::ChannelId::new(args.channel_id.get());
    let access = resolve_channel(&run.discord, run.guild_id, &run.invoker, channel_id)?;
    let required = if is_thread(access.channel.kind) {
        (
            serenity::Permissions::SEND_MESSAGES_IN_THREADS,
            "SEND_MESSAGES_IN_THREADS",
        )
    } else {
        (serenity::Permissions::SEND_MESSAGES, "SEND_MESSAGES")
    };
    if !access.permissions.contains(required.0) {
        bail!("invoker is missing {} in this channel", required.1);
    }
    if channel_id != run.channel_id {
        let preview = truncate_chars(&args.content.replace('\n', " "), 300);
        run.approve(
            "discord_send_message",
            &format!("Send to #{}:\n> {preview}", access.channel.name),
        )
        .await?;
    }
    let mut builder = serenity::CreateMessage::new()
        .content(args.content)
        .allowed_mentions(serenity::CreateAllowedMentions::new());
    if let Some(reply_id) = args.reply_to_message_id {
        builder = builder.reference_message((channel_id, serenity::MessageId::new(reply_id.get())));
    }
    let message = channel_id
        .send_message(&run.discord, builder)
        .await
        .context("failed to send Discord message")?;
    Ok(ToolOutput::text(format!(
        "Sent message {}: {}",
        message.id,
        jump_link(run.guild_id, channel_id, message.id)
    )))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct AddReaction {
    channel_id: Option<Snowflake>,
    message_id: Snowflake,
    emoji: String,
}

async fn add_reaction(run: Arc<Run>, args: AddReaction) -> Result<ToolOutput> {
    let channel_id = channel_id(&run, args.channel_id);
    require_permissions(
        &run.discord,
        run.guild_id,
        &run.invoker,
        channel_id,
        &[(serenity::Permissions::ADD_REACTIONS, "ADD_REACTIONS")],
    )?;
    let message_id = serenity::MessageId::new(args.message_id.get());
    let emoji = parse_emoji(&args.emoji)?;
    channel_id
        .create_reaction(&run.discord, message_id, emoji)
        .await
        .context("failed to add Discord reaction")?;
    Ok(ToolOutput::text(format!(
        "Reacted to message {message_id}: {}",
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

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateThread {
    channel_id: Option<Snowflake>,
    message_id: Option<Snowflake>,
    name: String,
    auto_archive_minutes: Option<u16>,
}

async fn create_thread(run: Arc<Run>, args: CreateThread) -> Result<ToolOutput> {
    if !(2..=100).contains(&args.name.chars().count()) {
        bail!("name must be between 2 and 100 characters");
    }
    let channel_id = channel_id(&run, args.channel_id);
    require_permissions(
        &run.discord,
        run.guild_id,
        &run.invoker,
        channel_id,
        &[(
            serenity::Permissions::CREATE_PUBLIC_THREADS,
            "CREATE_PUBLIC_THREADS",
        )],
    )?;
    let archive = match args.auto_archive_minutes.unwrap_or(DEFAULT_ARCHIVE_MINUTES) {
        60 => serenity::AutoArchiveDuration::OneHour,
        1_440 => serenity::AutoArchiveDuration::OneDay,
        4_320 => serenity::AutoArchiveDuration::ThreeDays,
        10_080 => serenity::AutoArchiveDuration::OneWeek,
        _ => bail!("auto_archive_minutes must be 60, 1440, 4320, or 10080"),
    };
    let builder = serenity::CreateThread::new(args.name)
        .auto_archive_duration(archive)
        .kind(serenity::ChannelType::PublicThread);
    let thread = match args.message_id {
        Some(id) => {
            channel_id
                .create_thread_from_message(
                    &run.discord,
                    serenity::MessageId::new(id.get()),
                    builder,
                )
                .await
        }
        None => channel_id.create_thread(&run.discord, builder).await,
    }
    .context("failed to create Discord thread")?;
    let link = channel_link(run.guild_id, thread.id);
    Ok(ToolOutput::text(format!(
        "Created thread {}: {link}",
        thread.id
    )))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct PinMessage {
    channel_id: Option<Snowflake>,
    message_id: Snowflake,
    pinned: bool,
}

async fn pin_message(run: Arc<Run>, args: PinMessage) -> Result<ToolOutput> {
    let channel_id = channel_id(&run, args.channel_id);
    let access = resolve_channel(&run.discord, run.guild_id, &run.invoker, channel_id)?;
    if !access
        .permissions
        .intersects(PIN_MESSAGES | serenity::Permissions::MANAGE_MESSAGES)
    {
        bail!("invoker is missing PIN_MESSAGES or MANAGE_MESSAGES in this channel");
    }
    let message_id = serenity::MessageId::new(args.message_id.get());
    if args.pinned {
        channel_id
            .pin(&run.discord, message_id)
            .await
            .context("failed to pin Discord message")?;
    } else {
        channel_id
            .unpin(&run.discord, message_id)
            .await
            .context("failed to unpin Discord message")?;
    }
    Ok(ToolOutput::text(format!(
        "{} message {message_id}: {}",
        if args.pinned { "Pinned" } else { "Unpinned" },
        jump_link(run.guild_id, channel_id, message_id)
    )))
}

#[cfg(test)]
mod tests;
