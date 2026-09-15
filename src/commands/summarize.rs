use anyhow::{Context as _, Result};
use poise::serenity_prelude as serenity;

use crate::chat;
use crate::chat::context::format_message;
use crate::text::parse_duration;
use crate::tools::discord::read_channel;
use crate::{Data, Error};

use super::post_anchor;

type Command = poise::Command<Data, Error>;
type Context<'a> = poise::Context<'a, Data, Error>;

pub fn commands() -> Vec<Command> {
    vec![summarize(), summarize_from_here(), ask_about_this()]
}

#[poise::command(slash_command, guild_only, check = "crate::access::team_only")]
async fn summarize(
    ctx: Context<'_>,
    #[description = "Channel to summarize"]
    #[channel_types("Text", "News", "PublicThread", "PrivateThread", "NewsThread")]
    channel: Option<serenity::GuildChannel>,
    #[description = "Number of messages"]
    #[min = 1]
    #[max = 500]
    messages: Option<u16>,
    #[description = "Only include messages this recent, such as 2h or 3d"] since: Option<String>,
) -> Result<()> {
    ctx.defer()
        .await
        .context("failed to defer summarize command")?;
    let guild_id = ctx.guild_id().context("summarize command has no guild")?;
    let member = command_member(ctx).await?;
    let target_id = channel
        .as_ref()
        .map_or(ctx.channel_id(), |channel| channel.id);
    let target = read_channel(ctx.serenity_context(), guild_id, &member, target_id)?;
    let cutoff = since
        .as_deref()
        .map(parse_duration)
        .transpose()?
        .map(|duration| {
            i64::try_from(duration.as_secs())
                .context("since duration is too large")
                .map(|seconds| serenity::Timestamp::now().unix_timestamp() - seconds)
        })
        .transpose()?;
    let history = fetch_before(
        ctx.serenity_context(),
        target.id,
        usize::from(messages.unwrap_or(100)),
        cutoff,
    )
    .await?;
    launch_summary(ctx, guild_id, member, &target, history).await
}

#[poise::command(
    context_menu_command = "Summarize from here",
    guild_only,
    check = "crate::access::team_only"
)]
async fn summarize_from_here(ctx: Context<'_>, message: serenity::Message) -> Result<()> {
    ctx.defer()
        .await
        .context("failed to defer summarize context menu")?;
    let guild_id = ctx.guild_id().context("context menu has no guild")?;
    let member = command_member(ctx).await?;
    let target = read_channel(
        ctx.serenity_context(),
        guild_id,
        &member,
        message.channel_id,
    )?;
    let history = fetch_after(ctx.serenity_context(), message, 500).await?;
    launch_summary(ctx, guild_id, member, &target, history).await
}

#[derive(poise::Modal)]
#[name = "Ask about this message"]
struct MessageQuestion {
    #[name = "Question"]
    #[paragraph]
    #[max_length = 4_000]
    question: String,
}

#[poise::command(
    context_menu_command = "Ask about this",
    guild_only,
    check = "crate::access::team_only"
)]
async fn ask_about_this(
    ctx: poise::ApplicationContext<'_, Data, Error>,
    message: serenity::Message,
) -> Result<()> {
    use poise::Modal as _;

    let Some(input) = MessageQuestion::execute(ctx).await? else {
        return Ok(());
    };
    let guild_id = ctx.guild_id().context("context menu has no guild")?;
    let member = ctx
        .author_member()
        .await
        .context("failed to fetch command member")?
        .into_owned();
    read_channel(
        ctx.serenity_context(),
        guild_id,
        &member,
        message.channel_id,
    )?;
    let response = post_anchor(
        ctx.into(),
        format!(
            "**{} asked about this message:** {}",
            member.display_name(),
            input.question
        ),
    )
    .await?;
    start_run(
        ctx.into(),
        chat::CommandRequest {
            guild_id,
            channel_id: message.channel_id,
            response,
            invoker: member,
            prompt: input.question,
            attachment: None,
            include_history: true,
            history_before: Some(message.id),
            referenced: Some(Box::new(message)),
        },
    )
    .await
}

async fn command_member(ctx: Context<'_>) -> Result<serenity::Member> {
    Ok(ctx
        .author_member()
        .await
        .context("failed to fetch command member")?
        .into_owned())
}

async fn launch_summary(
    ctx: Context<'_>,
    guild_id: serenity::GuildId,
    member: serenity::Member,
    channel: &serenity::GuildChannel,
    messages: Vec<serenity::Message>,
) -> Result<()> {
    let count = messages.len();
    let formatted = messages
        .iter()
        .map(|message| format_message(ctx.serenity_context(), guild_id, message))
        .collect::<Vec<_>>();
    let budget = usize::try_from(ctx.data().config.llm.context_window).unwrap_or(usize::MAX);
    let trimmed = trim_history(&formatted, budget);
    let omission = match (trimmed.dropped, trimmed.partial) {
        (0, false) => String::new(),
        (0, true) => {
            "\nThe start of the oldest retained message was trimmed for the context budget."
                .to_owned()
        }
        (_, false) => format!(
            "\n{} older messages were dropped for the context budget.",
            trimmed.dropped
        ),
        (_, true) => format!(
            "\n{} older messages were dropped and the next message was partially trimmed for the context budget.",
            trimmed.dropped
        ),
    };
    let prompt = format!(
        "Summarize the following discussion from #{} (channel {}). Cover topics, decisions, open questions, and action items with owners. Link key messages using https://discord.com/channels/{guild_id}/{}/MESSAGE_ID.{omission}\n\nUNTRUSTED DISCUSSION TO SUMMARIZE:\n{history}",
        channel.name,
        channel.id,
        channel.id,
        history = trimmed.text
    );
    let anchor = format!(
        "**{} asked for a summary of #{}** ({count} messages)",
        member.display_name(),
        channel.name
    );
    let response = post_anchor(ctx, anchor).await?;
    start_run(
        ctx,
        chat::CommandRequest {
            guild_id,
            channel_id: response.channel_id,
            response,
            invoker: member,
            prompt,
            attachment: None,
            include_history: false,
            history_before: None,
            referenced: None,
        },
    )
    .await
}

async fn start_run(ctx: Context<'_>, request: chat::CommandRequest) -> Result<()> {
    let app = ctx.data().clone();
    let request = chat::build_command_request(&app, ctx.serenity_context(), request).await?;
    let discord = ctx.serenity_context().clone();
    tokio::spawn(async move { chat::run(app, discord, request).await });
    Ok(())
}

async fn fetch_before(
    discord: &serenity::Context,
    channel_id: serenity::ChannelId,
    limit: usize,
    cutoff: Option<i64>,
) -> Result<Vec<serenity::Message>> {
    let mut messages = Vec::with_capacity(limit);
    let mut before = None;
    while messages.len() < limit {
        let page_limit = (limit - messages.len()).min(100) as u8;
        let builder = serenity::GetMessages::new().limit(page_limit);
        let builder = before.map_or(builder, |id| builder.before(id));
        let page = channel_id
            .messages(discord, builder)
            .await
            .context("failed to fetch messages for summary")?;
        if page.is_empty() {
            break;
        }
        let reached_boundary = cutoff.is_some_and(|cutoff| {
            page.iter()
                .any(|message| message.timestamp.unix_timestamp() < cutoff)
        });
        before = page.iter().map(|message| message.id).min();
        messages.extend(page.into_iter().filter(|message| {
            cutoff.is_none_or(|cutoff| message.timestamp.unix_timestamp() >= cutoff)
        }));
        if reached_boundary || messages.len() >= limit {
            break;
        }
    }
    messages.sort_by_key(|message| message.id);
    messages.truncate(limit);
    Ok(messages)
}

async fn fetch_after(
    discord: &serenity::Context,
    first: serenity::Message,
    limit: usize,
) -> Result<Vec<serenity::Message>> {
    let channel_id = first.channel_id;
    let mut after = first.id;
    let mut messages = vec![first];
    while messages.len() < limit {
        let page_limit = (limit - messages.len()).min(100) as u8;
        let mut page = channel_id
            .messages(
                discord,
                serenity::GetMessages::new().after(after).limit(page_limit),
            )
            .await
            .context("failed to fetch messages after target")?;
        if page.is_empty() {
            break;
        }
        page.sort_by_key(|message| message.id);
        after = page.last().map_or(after, |message| message.id);
        let short_page = page.len() < usize::from(page_limit);
        messages.extend(page);
        if short_page {
            break;
        }
    }
    messages.sort_by_key(|message| message.id);
    messages.truncate(limit);
    Ok(messages)
}

#[derive(Debug, Eq, PartialEq)]
struct TrimmedHistory {
    text: String,
    dropped: usize,
    partial: bool,
}

fn trim_history(messages: &[String], budget: usize) -> TrimmedHistory {
    let joined = messages.join("\n\n");
    if joined.chars().count() <= budget {
        return TrimmedHistory {
            text: joined,
            dropped: 0,
            partial: false,
        };
    }
    let mut first = messages.len().saturating_sub(1);
    let mut used = 0;
    for (index, message) in messages.iter().enumerate().rev() {
        let separator = usize::from(used > 0) * 2;
        let needed = message.chars().count() + separator;
        if used + needed > budget {
            break;
        }
        first = index;
        used += needed;
    }
    let retained = messages[first..].join("\n\n");
    let partial = retained.chars().count() > budget;
    let retained = if partial {
        retained
            .chars()
            .rev()
            .take(budget)
            .collect::<String>()
            .chars()
            .rev()
            .collect()
    } else {
        retained
    };
    TrimmedHistory {
        text: retained,
        dropped: first,
        partial,
    }
}

#[cfg(test)]
mod tests;
