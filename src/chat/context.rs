use std::sync::LazyLock;

use anyhow::{Context, Result};
use futures::future::join_all;
use poise::serenity_prelude as serenity;
use regex::{Captures, Regex};

use crate::App;
use crate::attachments::{self, Loaded};
use crate::llm::{ContentPart, ImageUrl, Message, UserContent};
use crate::text::truncate_chars;

const EMBED_DESCRIPTION_LIMIT: usize = 300;
const EMBED_FIELD_LIMIT: usize = 200;
const EMBED_FIELD_COUNT: usize = 10;
static CHANNEL_MENTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<#([0-9]+)>").expect("the channel mention pattern is valid"));
static ROLE_MENTION: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"<@&([0-9]+)>").expect("the role mention pattern is valid"));

pub struct ContextInput {
    pub before: serenity::MessageId,
    pub addressed_id: serenity::MessageId,
    pub timestamp: serenity::Timestamp,
    pub content: String,
    pub mentions: Vec<serenity::User>,
    pub attachments: Vec<serenity::Attachment>,
    pub referenced: Option<Box<serenity::Message>>,
}

pub async fn build(
    app: &App,
    discord: &serenity::Context,
    guild_id: serenity::GuildId,
    channel_id: serenity::ChannelId,
    invoker: &serenity::Member,
    include_history: bool,
    input: ContextInput,
) -> Result<Vec<Message>> {
    let mut history = if include_history {
        channel_id
            .messages(
                discord,
                serenity::GetMessages::new()
                    .before(input.before)
                    .limit(app.config.agent.history_messages),
            )
            .await
            .context("failed to fetch channel history")?
    } else {
        Vec::new()
    };
    history.reverse();

    let mut context_text = history
        .iter()
        .map(|message| format_message(discord, guild_id, message))
        .collect::<Vec<_>>()
        .join("\n\n");
    if let Some(referenced) = input.referenced.as_deref()
        && !history.iter().any(|message| message.id == referenced.id)
    {
        if !context_text.is_empty() {
            context_text.push_str("\n\n");
        }
        context_text.push_str("Referenced message outside recent history:\n");
        context_text.push_str(&format_message(discord, guild_id, referenced));
    }
    Ok(vec![
        addressed_message(app, discord, guild_id, invoker, input, context_text).await,
    ])
}

pub async fn continue_conversation(
    app: &App,
    discord: &serenity::Context,
    guild_id: serenity::GuildId,
    channel_id: serenity::ChannelId,
    invoker: &serenity::Member,
    last_message_id: serenity::MessageId,
    input: ContextInput,
) -> Result<Message> {
    let mut history = channel_id
        .messages(
            discord,
            serenity::GetMessages::new()
                .before(input.before)
                .limit(app.config.agent.history_messages),
        )
        .await
        .context("failed to fetch new channel messages for conversation")?;
    history.reverse();
    let new_messages = history
        .iter()
        .filter(|message| message.id > last_message_id)
        .map(|message| format_message(discord, guild_id, message))
        .collect::<Vec<_>>()
        .join("\n\n");
    let context_text = if new_messages.is_empty() {
        String::new()
    } else {
        format!("NEW MESSAGES SINCE THE LAST REPLY:\n{new_messages}")
    };
    Ok(addressed_message(app, discord, guild_id, invoker, input, context_text).await)
}

async fn addressed_message(
    app: &App,
    discord: &serenity::Context,
    guild_id: serenity::GuildId,
    invoker: &serenity::Member,
    input: ContextInput,
    mut context_text: String,
) -> Message {
    if !context_text.is_empty() {
        context_text.push_str("\n\n");
    }
    context_text.push_str("MESSAGE ADDRESSED TO BOT:\n");
    context_text.push_str(&format_header(
        &input.timestamp,
        invoker.display_name(),
        &invoker.user.name,
        invoker.user.id,
        input.addressed_id,
        input.referenced.as_deref().map(|message| message.id),
    ));
    context_text.push('\n');
    context_text.push_str(&replace_mentions(
        discord,
        guild_id,
        &input.content,
        &input.mentions,
    ));

    let referenced_attachments = input
        .referenced
        .as_deref()
        .map(|message| message.attachments.as_slice())
        .unwrap_or_default();
    let attachments = input
        .attachments
        .iter()
        .chain(referenced_attachments)
        .collect();
    let mut parts = vec![ContentPart::Text { text: context_text }];
    parts.extend(attachment_parts(app, attachments).await);
    Message::User {
        content: UserContent::from_parts(parts),
    }
}

async fn attachment_parts(app: &App, attachments: Vec<&serenity::Attachment>) -> Vec<ContentPart> {
    let loaded = join_all(
        attachments
            .iter()
            .map(|attachment| attachments::load(&app.http, attachment, app.config.llm.vision)),
    )
    .await;
    loaded
        .into_iter()
        .map(|item| match item {
            Loaded::Image(image) => ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: image.data_url(),
                },
            },
            Loaded::Text { name, text } => {
                let fence = if text.contains("```") { "````" } else { "```" };
                ContentPart::Text {
                    text: format!("\n[attachment: {name}]\n{fence}\n{text}\n{fence}"),
                }
            }
            Loaded::Unsupported { name, reason } => ContentPart::Text {
                text: format!("\n[attachment: {name}, unavailable: {reason}]"),
            },
        })
        .collect()
}

pub async fn steering_message(
    app: &App,
    content: &str,
    attachments: &[serenity::Attachment],
) -> Message {
    let mut parts = vec![ContentPart::Text {
        text: content.to_owned(),
    }];
    parts.extend(attachment_parts(app, attachments.iter().collect()).await);
    Message::User {
        content: UserContent::from_parts(parts),
    }
}

pub(crate) fn format_message(
    discord: &serenity::Context,
    guild_id: serenity::GuildId,
    message: &serenity::Message,
) -> String {
    let display_name = discord
        .cache
        .guild(guild_id)
        .and_then(|guild| {
            guild
                .members
                .get(&message.author.id)
                .map(|member| member.display_name().to_owned())
        })
        .unwrap_or_else(|| message.author.display_name().to_owned());
    let reply_to = message
        .referenced_message
        .as_deref()
        .map(|referenced| referenced.id)
        .or_else(|| {
            message
                .message_reference
                .as_ref()
                .and_then(|reference| reference.message_id)
        });
    let mut output = format_header(
        &message.timestamp,
        &display_name,
        &message.author.name,
        message.author.id,
        message.id,
        reply_to,
    );
    output.push('\n');
    let authored_by_bot = message.author.id == discord.cache.current_user().id;
    output.push_str(&replace_mentions(
        discord,
        guild_id,
        history_content(&message.content, authored_by_bot),
        &message.mentions,
    ));
    for attachment in &message.attachments {
        output.push_str(&format!(
            "\n[attachment: {}, {}, {} bytes]",
            attachment.filename,
            attachment.content_type.as_deref().unwrap_or("unknown type"),
            attachment.size
        ));
    }
    for embed in &message.embeds {
        let title = embed.title.as_deref().unwrap_or("untitled");
        let description = truncate_chars(
            embed.description.as_deref().unwrap_or(""),
            EMBED_DESCRIPTION_LIMIT,
        );
        let author = embed
            .author
            .as_ref()
            .map_or_else(String::new, |author| format!(" | by {}", author.name));
        let url = embed
            .url
            .as_deref()
            .map_or_else(String::new, |url| format!(" | {url}"));
        output.push_str(&format!("\n[embed: {title}{author}{url} | {description}]"));
        for field in embed.fields.iter().take(EMBED_FIELD_COUNT) {
            output.push_str(&format!(
                "\n[embed field: {}: {}]",
                field.name,
                truncate_chars(&field.value, EMBED_FIELD_LIMIT)
            ));
        }
        if let Some(footer) = &embed.footer {
            output.push_str(&format!("\n[embed footer: {}]", footer.text));
        }
        if embed.image.is_some() {
            output.push_str("\n[embed image]");
        }
        if embed.thumbnail.is_some() {
            output.push_str("\n[embed thumbnail]");
        }
    }
    for sticker in &message.sticker_items {
        output.push_str(&format!("\n[sticker: {}]", sticker.name));
    }
    if let Some(poll) = &message.poll {
        output.push_str(&format!(
            "\n[poll: {}]",
            poll.question.text.as_deref().unwrap_or("untitled")
        ));
        for answer in &poll.answers {
            output.push_str(&format!(
                "\n[poll answer: {}]",
                answer.poll_media.text.as_deref().unwrap_or("untitled")
            ));
        }
    }
    output
}

fn history_content(content: &str, authored_by_bot: bool) -> &str {
    if !authored_by_bot {
        return content;
    }
    let mut retained = content.trim_end_matches(['\r', '\n']);
    let mut removed = false;
    loop {
        let line_start = retained.rfind('\n').map_or(0, |index| index + 1);
        let line = retained[line_start..]
            .strip_suffix('\r')
            .unwrap_or(&retained[line_start..]);
        if !line.starts_with("-# ") {
            break;
        }
        removed = true;
        retained = retained[..line_start].trim_end_matches(['\r', '\n']);
    }
    if removed { retained } else { content }
}

fn format_header(
    timestamp: &serenity::Timestamp,
    display_name: &str,
    username: &str,
    user_id: serenity::UserId,
    message_id: serenity::MessageId,
    reply_to: Option<serenity::MessageId>,
) -> String {
    let time = timestamp.format("%Y-%m-%d %H:%M");
    let reply = reply_to.map_or_else(String::new, |id| format!(", reply to {id}"));
    format!("[{time} UTC] {display_name} (@{username}, user {user_id}) msg {message_id}{reply}:")
}

fn replace_mentions(
    discord: &serenity::Context,
    guild_id: serenity::GuildId,
    content: &str,
    mentions: &[serenity::User],
) -> String {
    let users = mentions.iter().fold(content.to_owned(), |text, user| {
        let replacement = format!("@{}({})", user.display_name(), user.id);
        text.replace(&format!("<@{}>", user.id), &replacement)
            .replace(&format!("<@!{}>", user.id), &replacement)
    });
    let channels = CHANNEL_MENTION
        .replace_all(&users, |captures: &Captures<'_>| {
            let raw = &captures[1];
            raw.parse::<u64>()
                .ok()
                .filter(|id| *id != 0)
                .and_then(|id| channel_name(discord, guild_id, serenity::ChannelId::new(id)))
                .map_or_else(
                    || format!("#unknown({raw})"),
                    |name| format!("#{name}({raw})"),
                )
        })
        .into_owned();
    ROLE_MENTION
        .replace_all(&channels, |captures: &Captures<'_>| {
            let raw = &captures[1];
            raw.parse::<u64>()
                .ok()
                .filter(|id| *id != 0)
                .and_then(|id| {
                    discord
                        .cache
                        .guild(guild_id)
                        .and_then(|guild| guild.roles.get(&serenity::RoleId::new(id)).cloned())
                })
                .map_or_else(
                    || format!("@unknown-role({raw})"),
                    |role| format!("@{}({raw})", role.name),
                )
        })
        .into_owned()
}

pub(crate) fn channel_name(
    discord: &serenity::Context,
    guild_id: serenity::GuildId,
    channel_id: serenity::ChannelId,
) -> Option<String> {
    discord.cache.guild(guild_id).and_then(|guild| {
        guild
            .channels
            .get(&channel_id)
            .or_else(|| guild.threads.iter().find(|thread| thread.id == channel_id))
            .map(|channel| channel.name.clone())
    })
}

#[cfg(test)]
mod tests;
