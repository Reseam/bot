use anyhow::{Context as _, Result};
use poise::serenity_prelude as serenity;

use crate::reminders::{self, NewReminder};
use crate::text::{DISCORD_MESSAGE_LIMIT, truncate_chars};
use crate::{Data, Error};

type Context<'a> = poise::Context<'a, Data, Error>;

#[poise::command(
    slash_command,
    guild_only,
    check = "crate::access::ai_access",
    subcommands("remind_set", "remind_list", "remind_cancel"),
    subcommand_required
)]
pub async fn remind(_ctx: Context<'_>) -> Result<()> {
    Ok(())
}

#[poise::command(
    slash_command,
    rename = "set",
    required_bot_permissions = "SEND_MESSAGES"
)]
async fn remind_set(
    ctx: Context<'_>,
    #[rename = "in"]
    #[description = "When, such as 30m, 2h, or 3d"]
    delay: String,
    #[description = "What to remind you about"]
    #[max_length = 1_000]
    message: String,
) -> Result<()> {
    let guild_id = ctx.guild_id().context("remind command has no guild")?;
    let due_at = reminders::schedule(
        &ctx.data().db,
        &NewReminder {
            guild_id,
            channel_id: ctx.channel_id(),
            user_id: ctx.author().id,
            message: &message,
        },
        &delay,
    )
    .await?;
    ctx.say(format!(
        "I'll remind you <t:{due_at}:R>: {}",
        message.trim()
    ))
    .await
    .context("failed to confirm reminder")?;
    Ok(())
}

#[poise::command(slash_command, ephemeral, rename = "list")]
async fn remind_list(ctx: Context<'_>) -> Result<()> {
    let guild_id = ctx.guild_id().context("remind command has no guild")?;
    let pending = reminders::list(&ctx.data().db, guild_id, ctx.author().id).await?;
    let text = if pending.is_empty() {
        "You have no reminders.".to_owned()
    } else {
        pending
            .iter()
            .map(|reminder| {
                Ok(format!(
                    "`{}` <t:{}:R> in <#{}>: {}",
                    reminder.id,
                    reminder.due_at,
                    reminder.channel_id()?,
                    truncate_chars(&reminder.message, 100)
                ))
            })
            .collect::<Result<Vec<_>>>()?
            .join("\n")
    };
    ctx.say(truncate_chars(&text, DISCORD_MESSAGE_LIMIT))
        .await
        .context("failed to send reminder list")?;
    Ok(())
}

#[poise::command(slash_command, ephemeral, rename = "cancel")]
async fn remind_cancel(
    ctx: Context<'_>,
    #[description = "Reminder to cancel"]
    #[autocomplete = "autocomplete_reminder"]
    reminder: i64,
) -> Result<()> {
    let guild_id = ctx.guild_id().context("remind command has no guild")?;
    let cancelled = reminders::cancel(&ctx.data().db, guild_id, ctx.author().id, reminder).await?;
    let reply = if cancelled {
        "Reminder cancelled."
    } else {
        "You have no reminder with that ID."
    };
    ctx.say(reply).await.context("failed to confirm cancel")?;
    Ok(())
}

async fn autocomplete_reminder(
    ctx: Context<'_>,
    partial: &str,
) -> serenity::CreateAutocompleteResponse {
    let Some(guild_id) = ctx.guild_id() else {
        return serenity::CreateAutocompleteResponse::new();
    };
    let pending = match reminders::list(&ctx.data().db, guild_id, ctx.author().id).await {
        Ok(pending) => pending,
        Err(error) => {
            tracing::warn!(error = %format!("{error:#}"), "failed to autocomplete reminders");
            return serenity::CreateAutocompleteResponse::new();
        }
    };
    let partial = partial.to_lowercase();
    let choices = pending
        .into_iter()
        .filter(|reminder| {
            reminder.id.to_string().starts_with(&partial)
                || reminder.message.to_lowercase().contains(&partial)
        })
        .map(|reminder| {
            serenity::AutocompleteChoice::new(
                truncate_chars(&format!("{}: {}", reminder.id, reminder.message), 100),
                reminder.id,
            )
        })
        .collect();
    serenity::CreateAutocompleteResponse::new().set_choices(choices)
}
