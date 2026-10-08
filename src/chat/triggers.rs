use std::sync::Arc;

use anyhow::{Context, Result};
use poise::serenity_prelude as serenity;
use tokio::sync::OwnedMutexGuard;
use tracing::{debug, error, info};

use super::{NewRun, RunRequest, context, guild_model, run, start_new};
use crate::access::has_ai_access;
use crate::{App, conversations};

const QUEUED: &str = "⏳";
const STEERED: &str = "👀";

pub async fn handle_message(
    app: &Arc<App>,
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
    let active = referenced.and_then(|referenced| app.runs.get(referenced.id));
    let is_bot_reply = referenced.is_some_and(|referenced| referenced.author.id == bot_id);
    if !mentions_bot(discord, message, guild_id, bot_id) && !is_bot_reply && active.is_none() {
        return Ok(());
    }
    let roles = message
        .member
        .as_deref()
        .map(|member| member.roles.as_slice())
        .unwrap_or_default();
    if !has_ai_access(&app.config, message.author.id, roles) {
        debug!(user_id = %message.author.id, "ignoring message from member without AI access");
        return Ok(());
    }

    let conversation_id = match (&active, referenced) {
        (Some(run), _) => Some(run.conversation_id),
        (None, Some(referenced)) => conversations::find_by_message(&app.db, referenced.id).await?,
        (None, None) => None,
    };
    if let Some(run) = conversation_id.and_then(|id| app.runs.get_conversation(id))
        && run.invoker.user.id == message.author.id
    {
        let steering = context::steering_message(
            app,
            &message.content,
            &message.attachments,
            run.model.config.vision,
        )
        .await;
        if run.steer(steering) {
            app.runs.register_message(message.id, &run);
            message
                .react(discord, reaction(STEERED))
                .await
                .context("failed to acknowledge steering message")?;
        }
        return Ok(());
    }

    let member = message
        .member(discord)
        .await
        .context("failed to fetch message author member")?;
    let input = context_input(message);
    let Some(conversation_id) = conversation_id else {
        return start_new(
            app,
            discord,
            NewRun {
                guild_id,
                channel_id: message.channel_id,
                invoker: member,
                include_history: true,
                input,
            },
        )
        .await;
    };

    let lock = app.runs.try_lock(conversation_id);
    if lock.is_none() {
        message
            .react(discord, reaction(QUEUED))
            .await
            .context("failed to mark queued message")?;
    }
    let (app, discord, message) = (app.clone(), discord.clone(), message.clone());
    tokio::spawn(async move {
        if let Err(error) =
            continue_conversation(&app, &discord, &message, member, conversation_id, lock).await
        {
            error!(error = %format!("{error:#}"), conversation_id, "failed to continue conversation");
        }
    });
    Ok(())
}

async fn continue_conversation(
    app: &Arc<App>,
    discord: &serenity::Context,
    message: &serenity::Message,
    member: serenity::Member,
    conversation_id: i64,
    lock: Option<OwnedMutexGuard<()>>,
) -> Result<()> {
    let lock = match lock {
        Some(lock) => lock,
        None => {
            let lock = app.runs.lock(conversation_id).await;
            message
                .delete_reaction(discord, None, reaction(QUEUED))
                .await
                .context("failed to clear queued marker")?;
            lock
        }
    };
    let guild_id = message
        .guild_id
        .context("continued conversation message has no guild")?;
    let conversation = conversations::load(&app.db, conversation_id).await?;
    let model = guild_model(app, guild_id).await;
    let last_message_id = conversation
        .last_message_id
        .context("continued conversation has no mapped Discord messages")?;
    let conversations::Conversation {
        mut transcript,
        prompt_prefix,
        sandbox_image,
        ..
    } = conversation;
    transcript.push(
        context::continue_conversation(
            app,
            discord,
            message.channel_id,
            &member,
            last_message_id,
            model.config.vision,
            context_input(message),
        )
        .await?,
    );
    run(
        app.clone(),
        discord.clone(),
        RunRequest {
            conversation_id,
            lock,
            guild_id,
            channel_id: message.channel_id,
            reply_to: message.id,
            invoker: member,
            model,
            transcript,
            prompt_prefix,
            sandbox_image,
        },
    )
    .await;
    Ok(())
}

fn context_input(message: &serenity::Message) -> context::ContextInput {
    context::ContextInput {
        before: message.id,
        addressed_id: message.id,
        timestamp: message.timestamp,
        content: message.content.clone(),
        mentions: message.mentions.clone(),
        attachments: message.attachments.clone(),
        referenced: message.referenced_message.clone(),
    }
}

fn reaction(emoji: &str) -> serenity::ReactionType {
    serenity::ReactionType::Unicode(emoji.to_owned())
}

fn mentions_bot(
    discord: &serenity::Context,
    message: &serenity::Message,
    guild_id: serenity::GuildId,
    bot_id: serenity::UserId,
) -> bool {
    message.mentions_user_id(bot_id)
        || discord.cache.guild(guild_id).is_some_and(|guild| {
            message
                .mention_roles
                .iter()
                .filter_map(|role_id| guild.roles.get(role_id))
                .any(|role| role.tags.bot_id == Some(bot_id))
        })
}

pub async fn handle_component(
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
    let Ok(id) = raw_id.parse::<u64>() else {
        return Ok(());
    };
    if let Some(run) = app.runs.get(serenity::MessageId::new(id)) {
        let user_id = interaction.user.id;
        let allowed = run.invoker.user.id == user_id
            || app.config.access.owner_ids.contains(&user_id)
            || member
                .permissions
                .is_some_and(|permissions| permissions.manage_messages());
        if !allowed {
            interaction
                .create_response(
                    discord,
                    serenity::CreateInteractionResponse::Message(
                        serenity::CreateInteractionResponseMessage::new()
                            .content("Only the person who started this run can stop it.")
                            .ephemeral(true),
                    ),
                )
                .await
                .context("failed to reject stop interaction")?;
            return Ok(());
        }
        info!(
            conversation_id = run.conversation_id,
            user = %interaction.user.name,
            %user_id,
            "run stopped"
        );
        run.cancel.cancel();
    }
    interaction
        .defer(discord)
        .await
        .context("failed to acknowledge stop interaction")?;
    Ok(())
}
