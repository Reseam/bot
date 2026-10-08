use anyhow::{Context as _, Result};
use poise::ChoiceParameter;

use crate::sandbox::SandboxKind;
use crate::{Data, Error, chat, settings};

type Context<'a> = poise::Context<'a, Data, Error>;

#[poise::command(
    slash_command,
    guild_only,
    ephemeral,
    check = "crate::access::team_only"
)]
pub async fn sandbox(
    ctx: Context<'_>,
    #[description = "Where team runs execute commands"] kind: Option<SandboxKind>,
) -> Result<()> {
    let guild_id = ctx.guild_id().context("sandbox command has no guild")?;
    let app = ctx.data();
    let reply = match kind {
        None => format!(
            "Team runs use `{}`. Members always use `just-bash`.",
            chat::guild_sandbox(app, guild_id).await.name()
        ),
        Some(SandboxKind::Modal) if app.config.sandbox.modal.is_none() => {
            "The real sandbox is not configured.".to_owned()
        }
        Some(kind) => {
            settings::set_sandbox(&app.db, guild_id, kind).await?;
            format!(
                "Team runs now use `{}`. It applies to new runs. Members always use `just-bash`.",
                kind.name()
            )
        }
    };
    ctx.send(poise::CreateReply::new().content(reply).ephemeral(true))
        .await
        .context("failed to send sandbox reply")?;
    Ok(())
}
