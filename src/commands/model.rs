use anyhow::{Context as _, Result};
use poise::serenity_prelude as serenity;

use crate::access::Tier;
use crate::text::{DISCORD_MESSAGE_LIMIT, truncate_chars};
use crate::{App, Data, Error, chat, settings};

type Context<'a> = poise::Context<'a, Data, Error>;

#[poise::command(slash_command, guild_only, owners_only, ephemeral)]
pub async fn model(
    ctx: Context<'_>,
    #[description = "Whose runs to switch"] tier: Option<Tier>,
    #[description = "Model to use, as provider/model"]
    #[autocomplete = "autocomplete_model"]
    model: Option<String>,
) -> Result<()> {
    let guild_id = ctx.guild_id().context("model command has no guild")?;
    let app = ctx.data();
    let reply = match (tier, model) {
        (_, None) => format!(
            "Team runs use `{}`.\nMember runs use `{}`.\n{}",
            chat::guild_model(app, guild_id, Tier::Team).await.key,
            chat::guild_model(app, guild_id, Tier::Member).await.key,
            available(app)
        ),
        (None, Some(_)) => {
            "Choose whose runs to switch with `tier`: `team` or `members`.".to_owned()
        }
        (Some(tier), Some(key)) => match app.llm.get(&key) {
            Some(model) => {
                settings::set_model(&app.db, guild_id, tier, &model.key).await?;
                let runs = match tier {
                    Tier::Team => "Team",
                    Tier::Member => "Member",
                };
                format!("{runs} runs now use `{key}`. It applies to new runs.")
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
