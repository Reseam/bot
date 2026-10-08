use anyhow::{Context as _, Result, bail};
use poise::serenity_prelude as serenity;

use super::post_anchor;
use crate::access;
use crate::chat::{self, context::ContextInput, context::format_message};
use crate::discord::{History, jump_link, read_channel, require_permissions, send_permission};
use crate::text::parse_duration;
use crate::{Data, Error};

const DEFAULT_MESSAGES: usize = 100;

type Command = poise::Command<Data, Error>;
type Context<'a> = poise::Context<'a, Data, Error>;

pub fn commands() -> Vec<Command> {
    vec![summarize(), summarize_from_here(), ask_about_this()]
}

#[poise::command(
    slash_command,
    guild_only,
    ephemeral,
    check = "crate::access::ai_access"
)]
async fn summarize(
    ctx: Context<'_>,
    #[description = "Channel to summarize"]
    #[channel_types("Text", "News", "PublicThread", "PrivateThread", "NewsThread")]
    channel: Option<serenity::GuildChannel>,
    #[description = "Number of messages (default: 100, or everything within since)"]
    #[min = 1]
    messages: Option<usize>,
    #[description = "Only include messages this recent, such as 2h or 3d"] since: Option<String>,
) -> Result<()> {
    ctx.defer_ephemeral()
        .await
        .context("failed to defer summarize command")?;
    let guild_id = ctx.guild_id().context("summarize command has no guild")?;
    let member = command_member(ctx).await?;
    let target_id = channel.map_or(ctx.channel_id(), |channel| channel.id);
    let target = read_channel(ctx.serenity_context(), guild_id, &member, target_id).await?;
    let cutoff = since
        .as_deref()
        .map(parse_duration)
        .transpose()?
        .map(|since| serenity::Timestamp::now().unix_timestamp() - since.as_secs() as i64);
    let limit = messages.unwrap_or(if cutoff.is_some() {
        usize::MAX
    } else {
        DEFAULT_MESSAGES
    });

    let budget = context_budget(ctx, &member).await;
    let mut history = History::before(target.id, None);
    let mut collected = Vec::new();
    let mut size = 0;
    let mut complete = true;
    'pages: loop {
        let page = history.next_page(ctx.serenity_context()).await?;
        if page.is_empty() {
            break;
        }
        for message in page {
            if cutoff.is_some_and(|cutoff| message.timestamp.unix_timestamp() < cutoff)
                || collected.len() == limit
            {
                break 'pages;
            }
            let formatted = format_message(ctx.serenity_context(), guild_id, &message);
            size += formatted.chars().count() + 2;
            if size > budget {
                complete = false;
                break 'pages;
            }
            collected.push(formatted);
        }
    }
    collected.reverse();
    launch(ctx, guild_id, member, &target, collected, complete).await
}

#[poise::command(
    context_menu_command = "Summarize from here",
    guild_only,
    ephemeral,
    check = "crate::access::ai_access"
)]
async fn summarize_from_here(ctx: Context<'_>, message: serenity::Message) -> Result<()> {
    ctx.defer_ephemeral()
        .await
        .context("failed to defer summarize context menu")?;
    let guild_id = ctx.guild_id().context("context menu has no guild")?;
    let member = command_member(ctx).await?;
    let target = read_channel(
        ctx.serenity_context(),
        guild_id,
        &member,
        message.channel_id,
    )
    .await?;

    let budget = context_budget(ctx, &member).await;
    let first = format_message(ctx.serenity_context(), guild_id, &message);
    let mut size = first.chars().count();
    let mut collected = vec![first];
    let mut history = History::after(message.channel_id, message.id);
    let mut complete = true;
    'pages: loop {
        let page = history.next_page(ctx.serenity_context()).await?;
        if page.is_empty() {
            break;
        }
        for message in page {
            let formatted = format_message(ctx.serenity_context(), guild_id, &message);
            size += formatted.chars().count() + 2;
            if size > budget {
                complete = false;
                break 'pages;
            }
            collected.push(formatted);
        }
    }
    launch(ctx, guild_id, member, &target, collected, complete).await
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
    check = "crate::access::ai_access"
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
    let member = command_member(ctx.into()).await?;
    read_channel(
        ctx.serenity_context(),
        guild_id,
        &member,
        message.channel_id,
    )
    .await?;
    let anchor = post_anchor(
        ctx.into(),
        format!(
            "**{} asked about {}:** {}",
            member.display_name(),
            jump_link(guild_id, message.channel_id, message.id),
            input.question
        ),
    )
    .await?;
    chat::start_new(
        ctx.data(),
        ctx.serenity_context(),
        chat::NewRun {
            guild_id,
            channel_id: message.channel_id,
            invoker: member,
            include_history: true,
            input: ContextInput {
                before: message.id,
                addressed_id: anchor.id,
                timestamp: anchor.timestamp,
                content: input.question,
                mentions: Vec::new(),
                attachments: Vec::new(),
                referenced: Some(Box::new(message)),
            },
        },
    )
    .await
}

async fn launch(
    ctx: Context<'_>,
    guild_id: serenity::GuildId,
    member: serenity::Member,
    channel: &serenity::GuildChannel,
    messages: Vec<String>,
    complete: bool,
) -> Result<()> {
    if messages.is_empty() {
        bail!("there are no messages to summarize");
    }
    require_permissions(
        ctx.serenity_context(),
        guild_id,
        &member,
        channel.id,
        send_permission(channel),
    )
    .await?;

    let count = messages.len();
    let omission = if complete {
        String::new()
    } else {
        "\nPart of the discussion was left out because it is longer than the model's context."
            .to_owned()
    };
    let prompt = format!(
        "Summarize the following discussion from #{} (channel {}). Cover topics, decisions, open questions, and action items with owners. Link key messages using https://discord.com/channels/{guild_id}/{}/MESSAGE_ID.{omission}\n\nUNTRUSTED DISCUSSION TO SUMMARIZE:\n{}",
        channel.name,
        channel.id,
        channel.id,
        messages.join("\n\n")
    );
    let anchor = channel
        .id
        .send_message(
            ctx.serenity_context(),
            serenity::CreateMessage::new()
                .content(format!(
                    "**{} asked for a summary** ({count} messages)",
                    member.display_name()
                ))
                .allowed_mentions(serenity::CreateAllowedMentions::new()),
        )
        .await
        .context("failed to post summary anchor")?;
    chat::start_new(
        ctx.data(),
        ctx.serenity_context(),
        chat::NewRun {
            guild_id,
            channel_id: channel.id,
            invoker: member,
            include_history: false,
            input: ContextInput {
                before: anchor.id,
                addressed_id: anchor.id,
                timestamp: anchor.timestamp,
                content: prompt,
                mentions: Vec::new(),
                attachments: Vec::new(),
                referenced: None,
            },
        },
    )
    .await?;
    ctx.send(poise::CreateReply::new().content(format!(
        "Summarizing in {}",
        jump_link(guild_id, channel.id, anchor.id)
    )))
    .await
    .context("failed to confirm summary")?;
    Ok(())
}

async fn context_budget(ctx: Context<'_>, member: &serenity::Member) -> usize {
    let app = ctx.data();
    let tier = access::tier(&app.config, member.user.id, &member.roles);
    let model = chat::guild_model(app, member.guild_id, tier).await;
    usize::try_from(model.config.context_window).unwrap_or(usize::MAX)
}

async fn command_member(ctx: Context<'_>) -> Result<serenity::Member> {
    Ok(ctx
        .author_member()
        .await
        .context("failed to fetch command member")?
        .into_owned())
}
