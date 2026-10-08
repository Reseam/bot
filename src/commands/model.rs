use anyhow::{Context as _, Result};
use poise::serenity_prelude as serenity;

use crate::text::{DISCORD_MESSAGE_LIMIT, truncate_chars};
use crate::{App, Data, Error, chat, settings};

type Context<'a> = poise::Context<'a, Data, Error>;

#[poise::command(slash_command, guild_only, owners_only, ephemeral)]
pub async fn model(
    ctx: Context<'_>,
    #[description = "Model to use, as provider/model"]
    #[autocomplete = "autocomplete_model"]
    model: Option<String>,
) -> Result<()> {
    let guild_id = ctx.guild_id().context("model command has no guild")?;
    let app = ctx.data();
    let reply = match model {
        None => format!(
            "Current model: `{}`\n{}",
            chat::guild_model(app, guild_id).await.key,
            available(app)
        ),
        Some(key) => match app.llm.get(&key) {
            Some(model) => {
                settings::set_model(&app.db, guild_id, &model.key).await?;
                format!("Model set to `{key}`. It applies to new runs.")
            }
            None => format!("Unknown model `{key}`.\n{}", available(app)),
        },
    };
    ctx.send(
        poise::CreateReply::new()
            .content(truncate_chars(&reply, DISCORD_MESSAGE_LIMIT))
            .ephemeral(true),
    )
    .await
    .context("failed to send model reply")?;
    Ok(())
}

fn available(app: &App) -> String {
    let models = app
        .llm
        .models()
        .iter()
        .map(|model| format!("`{}`", model.key))
        .collect::<Vec<_>>()
        .join(", ");
    format!("Available: {models}")
}

async fn autocomplete_model(
    ctx: Context<'_>,
    partial: &str,
) -> serenity::CreateAutocompleteResponse {
    let partial = partial.to_ascii_lowercase();
    let choices = ctx
        .data()
        .llm
        .models()
        .iter()
        .filter(|model| model.key.to_ascii_lowercase().contains(&partial))
        .take(25)
        .map(|model| serenity::AutocompleteChoice::from(model.key.clone()))
        .collect();
    serenity::CreateAutocompleteResponse::new().set_choices(choices)
}
