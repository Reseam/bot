use std::fmt::Write as _;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use base64::Engine;
use clap::Args;
use poise::serenity_prelude as serenity;

use super::channel;
use crate::attachments;
use crate::chat::Run;
use crate::chat::context::format_message;
use crate::cli::{CommandOutput, OutputFile, snowflake};
use crate::discord::{History, jump_link, read_channel};
use crate::text::{parse_duration, truncate_chars};

const DEFAULT_LIMIT: usize = 50;
const MAX_MESSAGES: usize = 10_000;
const EMBED_DESCRIPTION_LIMIT: usize = 2_000;

#[derive(Args)]
pub struct Messages {
    /// Channel or thread ID (default: the current channel)
    #[arg(long, value_parser = snowflake)]
    channel: Option<u64>,
    /// Number of messages (default: 50, or every message within --since)
    #[arg(long)]
    limit: Option<usize>,
    /// Only messages newer than this, such as 2h or 3d
    #[arg(long)]
    since: Option<String>,
    /// Read backwards from before this message ID
    #[arg(long, value_parser = snowflake, conflicts_with = "after")]
    before: Option<u64>,
    /// Read forwards from after this message ID
    #[arg(long, value_parser = snowflake)]
    after: Option<u64>,
}

#[derive(Args)]
pub struct Message {
    #[arg(value_parser = snowflake)]
    id: u64,
    /// Channel or thread ID (default: the current channel)
    #[arg(long, value_parser = snowflake)]
    channel: Option<u64>,
}

#[derive(Args)]
pub struct Attachment {
    /// Message ID
    #[arg(value_parser = snowflake)]
    message: u64,
    /// Channel or thread ID (default: the current channel)
    #[arg(long, value_parser = snowflake)]
    channel: Option<u64>,
    /// Attachment number as listed by `discord message`
    #[arg(long, default_value_t = 0)]
    index: usize,
    /// Save the original file to this path instead of printing its text
    #[arg(short, long)]
    output: Option<String>,
}

pub async fn messages(run: &Arc<Run>, args: Messages) -> Result<CommandOutput> {
    let channel_id = channel(run, args.channel);
    read_channel(&run.discord, run.guild_id, &run.invoker, channel_id).await?;
    let cutoff = args
        .since
        .as_deref()
        .map(parse_duration)
        .transpose()?
        .map(|since| serenity::Timestamp::now().unix_timestamp() - since.as_secs() as i64);
    let limit = args.limit.unwrap_or(if cutoff.is_some() {
        MAX_MESSAGES
    } else {
        DEFAULT_LIMIT
    });
    if !(1..=MAX_MESSAGES).contains(&limit) {
        bail!("--limit must be between 1 and {MAX_MESSAGES}");
    }
    let mut history = match args.after {
        Some(after) => History::after(channel_id, serenity::MessageId::new(after)),
        None => History::before(channel_id, args.before.map(serenity::MessageId::new)),
    };
    let mut messages = Vec::new();
    'pages: loop {
        if run.cancel.is_cancelled() {
            bail!("cancelled");
        }
        let page = history.next_page(&run.discord).await?;
        if page.is_empty() {
            break;
        }
        for message in page {
            if cutoff.is_some_and(|cutoff| message.timestamp.unix_timestamp() < cutoff) {
                if args.after.is_none() {
                    break 'pages;
                }
                continue;
            }
            messages.push(message);
            if messages.len() == limit {
                break 'pages;
            }
        }
    }
    messages.sort_by_key(|message| message.id);
    let text = messages
        .iter()
        .map(|message| format_message(&run.discord, run.guild_id, message))
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok(CommandOutput::text(if text.is_empty() {
        "No messages found.".to_owned()
    } else {
        text
    }))
}

pub async fn message(run: &Arc<Run>, args: Message) -> Result<CommandOutput> {
    let channel_id = channel(run, args.channel);
    read_channel(&run.discord, run.guild_id, &run.invoker, channel_id).await?;
    let message = channel_id
        .message(&run.discord, serenity::MessageId::new(args.id))
        .await
        .context("failed to read Discord message")?;
    let display_name = run
        .discord
        .cache
        .guild(run.guild_id)
        .and_then(|guild| guild.members.get(&message.author.id).cloned())
        .map_or_else(
            || message.author.display_name().to_owned(),
            |member| member.display_name().to_owned(),
        );
    let mut output = format!(
        "Author: {display_name} (@{}, {})\nCreated: {}",
        message.author.name, message.author.id, message.timestamp
    );
    if let Some(edited) = message.edited_timestamp {
        write!(output, "\nEdited: {edited}")?;
    }
    let content = message.content_safe(&run.discord.cache);
    write!(
        output,
        "\nContent: {}",
        if content.is_empty() {
            "(empty)"
        } else {
            &content
        }
    )?;
    if let Some(id) = message
        .message_reference
        .as_ref()
        .and_then(|reference| reference.message_id)
    {
        write!(output, "\nReply to: {id}")?;
    }
    for (index, attachment) in message.attachments.iter().enumerate() {
        write!(
            output,
            "\nAttachment {index}: {} | {} | {} bytes",
            attachment.filename,
            attachment.content_type.as_deref().unwrap_or("unknown type"),
            attachment.size
        )?;
    }
    for embed in &message.embeds {
        write!(
            output,
            "\nEmbed: {} | {} | {}",
            embed.title.as_deref().unwrap_or("untitled"),
            embed.url.as_deref().unwrap_or("no URL"),
            truncate_chars(
                embed.description.as_deref().unwrap_or(""),
                EMBED_DESCRIPTION_LIMIT
            )
        )?;
        for field in &embed.fields {
            write!(output, "\n  {}: {}", field.name, field.value)?;
        }
    }
    for reaction in &message.reactions {
        write!(
            output,
            "\nReaction: {} ×{}",
            reaction.reaction_type, reaction.count
        )?;
    }
    if let Some(thread) = &message.thread {
        write!(output, "\nStarted thread: #{} ({})", thread.name, thread.id)?;
    }
    write!(
        output,
        "\nPinned: {}\nJump link: {}",
        message.pinned,
        jump_link(run.guild_id, channel_id, message.id)
    )?;
    Ok(CommandOutput::text(output))
}

pub async fn attachment(run: &Arc<Run>, args: Attachment) -> Result<CommandOutput> {
    let channel_id = channel(run, args.channel);
    read_channel(&run.discord, run.guild_id, &run.invoker, channel_id).await?;
    let message = channel_id
        .message(&run.discord, serenity::MessageId::new(args.message))
        .await
        .context("failed to read Discord message")?;
    let attachment = message
        .attachments
        .get(args.index)
        .with_context(|| format!("message has no attachment at index {}", args.index))?;
    let Some(path) = args.output else {
        let text = attachments::text(&run.app.http, attachment)
            .await
            .with_context(|| format!("cannot print {} as text", attachment.filename))?;
        return Ok(CommandOutput::text(text));
    };
    let bytes = attachments::download_original(&run.app.http, attachment).await?;
    Ok(CommandOutput {
        stdout: format!(
            "Saved {} ({} bytes) to {path}\n",
            attachment.filename,
            bytes.len()
        ),
        file: Some(OutputFile {
            path,
            base64: base64::engine::general_purpose::STANDARD.encode(bytes),
        }),
        ..CommandOutput::default()
    })
}
