use std::time::Duration;

use anyhow::{Context as _, Result, bail};
use poise::serenity_prelude as serenity;

use crate::db::discord_id;
use crate::moderation::{actions, case_by_id, cases_for};
use crate::{Data, Error};

type Command = poise::Command<Data, Error>;
type Context<'a> = poise::Context<'a, Data, Error>;

pub fn all() -> Vec<Command> {
    vec![cases(), case(), case_delete(), modlog()]
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MODERATE_MEMBERS",
    required_permissions = "MODERATE_MEMBERS"
)]
async fn cases(ctx: Context<'_>, user: serenity::User) -> Result<()> {
    let guild_id = ctx.guild_id().context("cases command has no guild")?;
    let cases = cases_for(&ctx.data().db, guild_id, user.id).await?;
    let text = if cases.is_empty() {
        "No moderation cases found.".to_owned()
    } else {
        cases
            .iter()
            .map(|case| {
                format!(
                    "`#{}` **{}** <t:{}:d> by <@{}>: {}",
                    case.id, case.action, case.created_at, case.moderator_id, case.reason
                )
            })
            .collect::<Vec<_>>()
            .join("\n")
    };
    ephemeral(ctx, text).await
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MODERATE_MEMBERS",
    required_permissions = "MODERATE_MEMBERS"
)]
async fn case(ctx: Context<'_>, id: i64) -> Result<()> {
    let guild_id = ctx.guild_id().context("case command has no guild")?;
    let case = case_by_id(&ctx.data().db, guild_id, id)
        .await?
        .context("moderation case not found")?;
    let duration = case.duration_secs.map_or_else(
        || "None".to_owned(),
        |seconds| humantime::format_duration(Duration::from_secs(seconds as u64)).to_string(),
    );
    let expiry = case.expires_at.map_or_else(
        || "None".to_owned(),
        |timestamp| format!("<t:{timestamp}:F>"),
    );
    let targets = [
        case.target_id.map(|id| format!("Target: <@{id}> (`{id}`)")),
        case.channel_id
            .map(|id| format!("Channel: <#{id}> (`{id}`)")),
    ]
    .into_iter()
    .flatten()
    .collect::<Vec<_>>()
    .join("\n");
    ephemeral(
        ctx,
        format!(
            "**Case #{}**\nAction: {}\n{}\nModerator: <@{}>\nReason: {}\nCreated: <t:{}:F>\nDuration: {}\nExpiry: {}\nResolved: {}",
            case.id,
            case.action,
            targets,
            case.moderator_id,
            case.reason,
            case.created_at,
            duration,
            expiry,
            case.resolved,
        ),
    )
    .await
}

#[poise::command(
    rename = "case-delete",
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD"
)]
async fn case_delete(ctx: Context<'_>, id: i64) -> Result<()> {
    let guild_id = ctx.guild_id().context("case-delete command has no guild")?;
    let affected = sqlx::query("DELETE FROM mod_cases WHERE guild_id = ? AND id = ?")
        .bind(discord_id(guild_id.get())?)
        .bind(id)
        .execute(&ctx.data().db)
        .await
        .context("failed to delete moderation case")?
        .rows_affected();
    if affected == 0 {
        bail!("moderation case not found");
    }
    ctx.say(format!("Deleted case #{id}.")).await?;
    Ok(())
}

#[poise::command(
    slash_command,
    guild_only,
    default_member_permissions = "MANAGE_GUILD",
    required_permissions = "MANAGE_GUILD"
)]
async fn modlog(ctx: Context<'_>, channel: Option<serenity::GuildChannel>) -> Result<()> {
    let guild_id = ctx.guild_id().context("modlog command has no guild")?;
    if let Some(channel) = channel {
        actions::set_mod_log(&ctx.data().db, guild_id, channel.id).await?;
        ctx.say(format!("Moderation log set to <#{}>.", channel.id))
            .await?;
        return Ok(());
    }
    let current = sqlx::query_scalar::<_, Option<i64>>(
        "SELECT mod_log_channel_id FROM guild_settings WHERE guild_id = ?",
    )
    .bind(discord_id(guild_id.get())?)
    .fetch_optional(&ctx.data().db)
    .await
    .context("failed to read moderation log channel")?
    .flatten();
    ephemeral(
        ctx,
        current.map_or_else(
            || "No moderation log channel is configured.".to_owned(),
            |id| format!("Moderation log: <#{id}>."),
        ),
    )
    .await
}

async fn ephemeral(ctx: Context<'_>, content: String) -> Result<()> {
    ctx.send(poise::CreateReply::new().content(content).ephemeral(true))
        .await?;
    Ok(())
}
