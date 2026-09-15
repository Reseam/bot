use anyhow::{Context as _, Result};
use poise::serenity_prelude as serenity;

use crate::chat;
use crate::text::{DISCORD_MESSAGE_LIMIT, truncate_chars};
use crate::{Data, Error};

mod summarize;

type Command = poise::Command<Data, Error>;
type Context<'a> = poise::Context<'a, Data, Error>;

pub fn all() -> Vec<Command> {
    let mut commands = vec![ask()];
    commands.extend(summarize::commands());
    commands
}

pub(super) async fn post_anchor(ctx: Context<'_>, text: String) -> Result<serenity::Message> {
    let reply = ctx
        .send(
            poise::CreateReply::new()
                .content(truncate_chars(&text, DISCORD_MESSAGE_LIMIT))
                .allowed_mentions(serenity::CreateAllowedMentions::new()),
        )
        .await
        .context("failed to post command anchor")?;
    reply
        .into_message()
        .await
        .context("failed to fetch command anchor")
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
    let response = post_anchor(
        ctx,
        format!("**{} asked:** {prompt}", member.display_name()),
    )
    .await?;
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
            history_before: None,
            referenced: None,
        },
    )
    .await?;
    let discord = ctx.serenity_context().clone();
    tokio::spawn(async move { chat::run(app, discord, request).await });
    Ok(())
}
