use anyhow::{Context as _, Result};
use poise::serenity_prelude as serenity;

use crate::chat;
use crate::text::{DISCORD_MESSAGE_LIMIT, truncate_chars};
use crate::{Data, Error};

type Command = poise::Command<Data, Error>;
type Context<'a> = poise::Context<'a, Data, Error>;

pub fn all() -> Vec<Command> {
    vec![ask()]
}

#[poise::command(slash_command, guild_only, check = "crate::access::team_only")]
async fn ask(
    ctx: Context<'_>,
    #[description = "What to ask"] prompt: String,
    #[description = "Optional file"] file: Option<serenity::Attachment>,
) -> Result<()> {
    let guild_id = ctx.guild_id().context("ask command has no guild")?;
    let member = ctx
        .author_member()
        .await
        .context("failed to fetch command member")?
        .into_owned();
    let heading = format!("**{} asked:** ", member.display_name());
    let available = DISCORD_MESSAGE_LIMIT.saturating_sub(heading.chars().count());
    let visible_prompt = truncate_chars(&prompt, available);
    let reply = ctx
        .send(
            poise::CreateReply::new()
                .content(format!("{heading}{visible_prompt}"))
                .allowed_mentions(serenity::CreateAllowedMentions::new()),
        )
        .await
        .context("failed to post ask prompt")?;
    let response = reply
        .into_message()
        .await
        .context("failed to fetch ask response")?;
    let app = ctx.data().clone();
    let request = chat::build_command_request(
        &app,
        ctx.serenity_context(),
        chat::CommandRequest {
            guild_id,
            channel_id: ctx.channel_id(),
            response,
            invoker: member,
            prompt,
            attachment: file,
        },
    )
    .await?;
    let discord = ctx.serenity_context().clone();
    tokio::spawn(async move { chat::run(app, discord, request).await });
    Ok(())
}
