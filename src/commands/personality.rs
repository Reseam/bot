use anyhow::{Context as _, Result};

use crate::settings;
use crate::{Data, Error};

#[derive(poise::Modal)]
#[name = "Bot personality"]
struct Personality {
    #[name = "Instructions"]
    #[placeholder = "Tone, style, and anything the bot should keep in mind. Leave empty to clear."]
    #[paragraph]
    #[max_length = 4_000]
    instructions: Option<String>,
}

#[poise::command(slash_command, guild_only, owners_only, ephemeral)]
pub async fn personality(ctx: poise::ApplicationContext<'_, Data, Error>) -> Result<()> {
    let guild_id = ctx.guild_id().context("personality command has no guild")?;
    let db = &ctx.data().db;
    let current = settings::personality(db, guild_id).await?;
    let Some(input) = poise::execute_modal(
        ctx,
        Some(Personality {
            instructions: current,
        }),
        None,
    )
    .await?
    else {
        return Ok(());
    };
    let instructions = input
        .instructions
        .as_deref()
        .map(str::trim)
        .filter(|text| !text.is_empty());
    settings::set_personality(db, guild_id, instructions).await?;
    let reply = if instructions.is_some() {
        "Personality saved. It applies to new runs."
    } else {
        "Personality cleared."
    };
    ctx.send(poise::CreateReply::new().content(reply).ephemeral(true))
        .await
        .context("failed to confirm personality")?;
    Ok(())
}
