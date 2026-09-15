use std::sync::Arc;

use anyhow::{Context, Result};
use poise::serenity_prelude as serenity;
use tracing::debug;

use super::{RunRequest, context, run};
use crate::access::is_team;
use crate::conversations;
use crate::{App, Data, Error};

pub async fn event_handler(
    framework: poise::FrameworkContext<'_, Data, Error>,
    event: &serenity::FullEvent,
) -> Result<()> {
    match event {
        serenity::FullEvent::Message { new_message } => {
            handle_message(
                framework.user_data.clone(),
                framework.serenity_context,
                new_message,
            )
            .await
        }
        serenity::FullEvent::InteractionCreate { interaction } => {
            if let Some(component) = interaction.as_message_component() {
                handle_component(framework.user_data, framework.serenity_context, component)
                    .await?;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

async fn handle_message(
    app: Arc<App>,
    discord: &serenity::Context,
    message: &serenity::Message,
) -> Result<()> {
    let Some(guild_id) = message.guild_id else {
        return Ok(());
    };
    if message.author.bot || message.webhook_id.is_some() {
        return Ok(());
    }

    let bot_id = discord.cache.current_user().id;
    let referenced = message.referenced_message.as_deref();
    let direct_active = referenced.and_then(|message| app.runs.get(message.id));
    let is_bot_reply = referenced.is_some_and(|message| message.author.id == bot_id);
    if !message.mentions_user_id(bot_id) && !is_bot_reply && direct_active.is_none() {
        return Ok(());
    }
    let roles = message
        .member
        .as_deref()
        .map(|member| member.roles.as_slice())
        .unwrap_or_default();
    if !is_team(&app.config, message.author.id, roles) {
        debug!(user_id = %message.author.id, "ignoring message from non-team member");
        return Ok(());
    }

    let conversation = match referenced {
        Some(referenced) => conversations::find_by_message(&app.db, referenced.id).await?,
        None => None,
    };
    let active = direct_active.or_else(|| {
        conversation
            .as_ref()
            .and_then(|conversation| app.runs.get_conversation(conversation.id))
    });

    if let Some(handle) = active
        && handle.invoker == message.author.id
    {
        let steering =
            context::steering_message(&app, &message.content, &message.attachments).await;
        if handle.steering.send(steering).is_ok() {
            app.runs.register_message(message.id, handle);
            message
                .react(discord, serenity::ReactionType::Unicode("👀".to_owned()))
                .await
                .context("failed to acknowledge steering message")?;
        }
        return Ok(());
    }

    let member = message
        .member(discord)
        .await
        .context("failed to fetch message author member")?;
    let input = context::ContextInput {
        before: message.id,
        addressed_id: message.id,
        timestamp: message.timestamp,
        content: message.content.clone(),
        mentions: message.mentions.clone(),
        attachments: message.attachments.clone(),
        referenced: message.referenced_message.clone(),
    };
    let (conversation_id, transcript) = match conversation {
        Some(mut conversation) => {
            let last_message_id = conversations::last_message_id(&app.db, conversation.id)
                .await?
                .context("continued conversation has no mapped Discord messages")?;
            conversation.transcript.push(
                context::continue_conversation(
                    &app,
                    discord,
                    guild_id,
                    message.channel_id,
                    &member,
                    last_message_id,
                    input,
                )
                .await?,
            );
            (Some(conversation.id), conversation.transcript)
        }
        None => (
            None,
            context::build(
                &app,
                discord,
                guild_id,
                message.channel_id,
                &member,
                true,
                input,
            )
            .await?,
        ),
    };
    let request = RunRequest {
        channel_id: message.channel_id,
        guild_id,
        reply_to: message.id,
        invoker: member,
        transcript,
        conversation_id,
        message_ids: vec![message.id],
    };
    let discord = discord.clone();
    tokio::spawn(async move { run(app, discord, request).await });
    Ok(())
}

async fn handle_component(
    app: &Arc<App>,
    discord: &serenity::Context,
    interaction: &serenity::ComponentInteraction,
) -> Result<()> {
    let Some(raw_id) = interaction.data.custom_id.strip_prefix("stop:") else {
        return Ok(());
    };
    let Some(member) = interaction.member.as_ref() else {
        return Ok(());
    };
    if !is_team(&app.config, interaction.user.id, &member.roles) {
        interaction
            .create_response(
                discord,
                serenity::CreateInteractionResponse::Message(
                    serenity::CreateInteractionResponseMessage::new()
                        .content("Only the Reseam team can use this.")
                        .ephemeral(true),
                ),
            )
            .await
            .context("failed to reject stop interaction")?;
        return Ok(());
    }
    let Ok(id) = raw_id.parse::<u64>() else {
        return Ok(());
    };
    if let Some(handle) = app.runs.get(serenity::MessageId::new(id)) {
        handle.cancel.cancel();
    }
    interaction
        .defer(discord)
        .await
        .context("failed to acknowledge stop interaction")?;
    Ok(())
}
