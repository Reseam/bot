use std::fmt::Write as _;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use poise::serenity_prelude as serenity;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{channel_id, is_thread, jump_link, read_channel};
use crate::attachments::{self, Loaded};
use crate::chat::Run;
use crate::chat::context::format_message;
use crate::text::truncate_chars;
use crate::tools::{Snowflake, Tool, ToolOutput};

mod member;

const DEFAULT_MESSAGE_LIMIT: u8 = 50;
const EMBED_DESCRIPTION_LIMIT: usize = 2_000;
const CHANNEL_TOPIC_LIMIT: usize = 200;

pub fn tools(run: &Arc<Run>) -> Vec<Tool> {
    let mut tools = vec![
        Tool::new(
            "discord_read_messages",
            "Read messages from a Discord channel when recent conversation context is insufficient. Results are returned oldest first.",
            run.clone(),
            read_messages,
        ),
        Tool::new(
            "discord_get_message",
            "Get complete details for one Discord message when its content, attachments, embeds, reactions, or metadata are needed.",
            run.clone(),
            get_message,
        ),
        Tool::new(
            "discord_list_channels",
            "List the server's categories, visible channels, and active threads when you need to find or identify a channel.",
            run.clone(),
            list_channels,
        ),
        Tool::new(
            "discord_server_info",
            "Get server metadata, roles, features, and expression counts when answering questions about the Discord server.",
            run.clone(),
            server_info,
        ),
        Tool::new(
            "discord_view_attachment",
            "Load an attachment from a Discord message when its contents are needed to answer the request. Images are returned as image input and documents as extracted text.",
            run.clone(),
            view_attachment,
        ),
    ];
    tools.extend(member::tools(run));
    tools
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct NoArguments {}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadMessages {
    channel_id: Option<Snowflake>,
    before: Option<Snowflake>,
    after: Option<Snowflake>,
    around: Option<Snowflake>,
    limit: Option<u8>,
}

async fn read_messages(run: Arc<Run>, args: ReadMessages) -> Result<ToolOutput> {
    let selected = usize::from(args.before.is_some())
        + usize::from(args.after.is_some())
        + usize::from(args.around.is_some());
    if selected > 1 {
        bail!("only one of before, after, or around may be provided");
    }
    let limit = args.limit.unwrap_or(DEFAULT_MESSAGE_LIMIT);
    if !(1..=100).contains(&limit) {
        bail!("limit must be between 1 and 100");
    }
    let channel_id = channel_id(&run, args.channel_id);
    read_channel(&run.discord, run.guild_id, &run.invoker, channel_id)?;
    let builder = serenity::GetMessages::new().limit(limit);
    let builder = if let Some(id) = args.before {
        builder.before(serenity::MessageId::new(id.get()))
    } else if let Some(id) = args.after {
        builder.after(serenity::MessageId::new(id.get()))
    } else if let Some(id) = args.around {
        builder.around(serenity::MessageId::new(id.get()))
    } else {
        builder
    };
    let mut messages = channel_id
        .messages(&run.discord, builder)
        .await
        .context("failed to read Discord messages")?;
    messages.sort_by_key(|message| message.id);
    let output = messages
        .iter()
        .map(|message| format_message(&run.discord, run.guild_id, message))
        .collect::<Vec<_>>()
        .join("\n\n");
    Ok(ToolOutput::text(if output.is_empty() {
        "No messages found.".to_owned()
    } else {
        output
    }))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetMessage {
    channel_id: Option<Snowflake>,
    message_id: Snowflake,
}

async fn get_message(run: Arc<Run>, args: GetMessage) -> Result<ToolOutput> {
    let channel_id = channel_id(&run, args.channel_id);
    read_channel(&run.discord, run.guild_id, &run.invoker, channel_id)?;
    let message = channel_id
        .message(
            &run.discord,
            serenity::MessageId::new(args.message_id.get()),
        )
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
    if let Some(reference) = message.message_reference.as_ref()
        && let Some(id) = reference.message_id
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
    Ok(ToolOutput::text(output))
}

async fn list_channels(run: Arc<Run>, _args: NoArguments) -> Result<ToolOutput> {
    let guild = run
        .discord
        .cache
        .guild(run.guild_id)
        .context("server is not available in the Discord cache")?;
    let mut channels = guild.channels.values().cloned().collect::<Vec<_>>();
    let mut threads = guild.threads.clone();
    channels.sort_by_key(|channel| (channel.position, channel.id));
    threads.sort_by_key(|channel| (channel.position, channel.id));
    let visible = |channel: &serenity::GuildChannel| {
        let permission_channel = if is_thread(channel.kind) {
            channel.parent_id.and_then(|id| guild.channels.get(&id))
        } else {
            Some(channel)
        };
        permission_channel.is_some_and(|channel| {
            guild
                .user_permissions_in(channel, &run.invoker)
                .view_channel()
        })
    };
    let mut output = String::new();
    for category in channels
        .iter()
        .filter(|channel| channel.kind == serenity::ChannelType::Category && visible(channel))
    {
        writeln!(output, "Category: {} ({})", category.name, category.id)?;
        for channel in channels
            .iter()
            .filter(|channel| channel.parent_id == Some(category.id) && visible(channel))
        {
            writeln!(
                output,
                "  {} #{} ({}){}",
                channel.kind.name(),
                channel.name,
                channel.id,
                topic(channel)
            )?;
        }
    }
    for channel in channels.iter().filter(|channel| {
        channel.kind != serenity::ChannelType::Category
            && channel.parent_id.is_none()
            && visible(channel)
    }) {
        writeln!(
            output,
            "{} #{} ({}){}",
            channel.kind.name(),
            channel.name,
            channel.id,
            topic(channel)
        )?;
    }
    if threads.iter().any(visible) {
        output.push_str("Active threads:\n");
        for thread in threads.iter().filter(|thread| visible(thread)) {
            writeln!(
                output,
                "  #{} ({}, parent {})",
                thread.name,
                thread.id,
                thread
                    .parent_id
                    .map_or_else(|| "unknown".to_owned(), |id| id.to_string())
            )?;
        }
    }
    Ok(ToolOutput::text(output.trim_end().to_owned()))
}

fn topic(channel: &serenity::GuildChannel) -> String {
    channel.topic.as_deref().map_or_else(String::new, |topic| {
        format!(" | {}", truncate_chars(topic, CHANNEL_TOPIC_LIMIT))
    })
}

async fn server_info(run: Arc<Run>, _args: NoArguments) -> Result<ToolOutput> {
    let guild = run
        .discord
        .cache
        .guild(run.guild_id)
        .context("server is not available in the Discord cache")?;
    let mut roles = guild.roles.values().collect::<Vec<_>>();
    roles.sort_by_key(|role| std::cmp::Reverse((role.position, role.id)));
    let mut output = format!(
        "Name: {}\nID: {}\nOwner ID: {}\nCreated: {}\nApproximate members: {}\nDescription: {}\nBoost tier: {:?}\nBoosts: {}\nEmoji count: {}\nSticker count: {}\nFeatures: {}\nRoles:",
        guild.name,
        guild.id,
        guild.owner_id,
        guild.id.created_at(),
        guild.approximate_member_count.unwrap_or(guild.member_count),
        guild.description.as_deref().unwrap_or("none"),
        guild.premium_tier,
        guild.premium_subscription_count.unwrap_or(0),
        guild.emojis.len(),
        guild.stickers.len(),
        guild.features.join(", ")
    );
    for role in roles {
        write!(
            output,
            "\n- {} ({}) | position {} | managed {}",
            role.name, role.id, role.position, role.managed
        )?;
    }
    Ok(ToolOutput::text(output))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ViewAttachment {
    channel_id: Option<Snowflake>,
    message_id: Snowflake,
    index: Option<usize>,
}

async fn view_attachment(run: Arc<Run>, args: ViewAttachment) -> Result<ToolOutput> {
    let channel_id = channel_id(&run, args.channel_id);
    read_channel(&run.discord, run.guild_id, &run.invoker, channel_id)?;
    let message = channel_id
        .message(
            &run.discord,
            serenity::MessageId::new(args.message_id.get()),
        )
        .await
        .context("failed to read Discord message")?;
    let index = args.index.unwrap_or(0);
    let attachment = message
        .attachments
        .get(index)
        .with_context(|| format!("message has no attachment at index {index}"))?;
    match attachments::load(&run.app.http, attachment, run.app.config.llm.vision).await {
        Loaded::Image(image) => Ok(ToolOutput {
            text: format!("Loaded attachment {}.", attachment.filename),
            images: vec![image],
        }),
        Loaded::Text { text, .. } => Ok(ToolOutput::text(text)),
        Loaded::Unsupported { name, reason } => bail!("could not load attachment {name}: {reason}"),
    }
}
