use std::sync::Arc;

use anyhow::{Context, Result, bail};
use poise::serenity_prelude as serenity;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{Snowflake, Tool, ToolOutput};
use crate::attachments::{self, Loaded};
use crate::chat::Run;
use crate::chat::context::format_message;

const DEFAULT_MESSAGE_LIMIT: u8 = 50;

pub fn tools(run: &Arc<Run>) -> Vec<Tool> {
    vec![
        Tool::new::<ReadMessages, _, _, _>(
            "discord_read_messages",
            "Read messages from a Discord channel when recent conversation context is insufficient. Results are returned oldest first.",
            run.clone(),
            read_messages,
        ),
        Tool::new::<ViewAttachment, _, _, _>(
            "discord_view_attachment",
            "Load an attachment from a Discord message when its contents are needed to answer the request. Images are returned as image input and documents as extracted text.",
            run.clone(),
            view_attachment,
        ),
    ]
}

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
    require_read_access(&run, channel_id)?;
    let messages = serenity::GetMessages::new().limit(limit);
    let messages = if let Some(id) = args.before {
        messages.before(serenity::MessageId::new(id.get()))
    } else if let Some(id) = args.after {
        messages.after(serenity::MessageId::new(id.get()))
    } else if let Some(id) = args.around {
        messages.around(serenity::MessageId::new(id.get()))
    } else {
        messages
    };
    let mut messages = channel_id
        .messages(&run.discord, messages)
        .await
        .context("failed to read Discord messages")?;
    messages.reverse();
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
struct ViewAttachment {
    channel_id: Option<Snowflake>,
    message_id: Snowflake,
    index: Option<usize>,
}

async fn view_attachment(run: Arc<Run>, args: ViewAttachment) -> Result<ToolOutput> {
    let channel_id = channel_id(&run, args.channel_id);
    require_read_access(&run, channel_id)?;
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

fn channel_id(run: &Run, requested: Option<Snowflake>) -> serenity::ChannelId {
    requested.map_or(run.channel_id, |id| serenity::ChannelId::new(id.get()))
}

fn require_read_access(run: &Run, channel_id: serenity::ChannelId) -> Result<()> {
    let guild = run
        .discord
        .cache
        .guild(run.guild_id)
        .context("server is not available in the Discord cache")?;
    let (channel, permission_channel) = if let Some(channel) = guild.channels.get(&channel_id) {
        (channel, channel)
    } else if let Some(thread) = guild.threads.iter().find(|thread| thread.id == channel_id) {
        let parent_id = thread.parent_id.context("thread has no parent channel")?;
        let parent = guild
            .channels
            .get(&parent_id)
            .context("thread parent is not available in the Discord cache")?;
        (thread, parent)
    } else {
        bail!("channel does not belong to this server");
    };
    if channel.guild_id != run.guild_id {
        bail!("channel does not belong to this server");
    }
    let permissions = guild.user_permissions_in(permission_channel, &run.invoker);
    if !permissions.view_channel() {
        bail!("invoker is missing VIEW_CHANNEL in this channel");
    }
    if !permissions.read_message_history() {
        bail!("invoker is missing READ_MESSAGE_HISTORY in this channel");
    }
    Ok(())
}
