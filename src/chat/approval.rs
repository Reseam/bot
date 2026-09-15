use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result};
use futures::StreamExt;
use poise::serenity_prelude as serenity;
use tracing::warn;

use super::Run;
use crate::text::truncate_chars;

const APPROVAL_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const EMBED_DESCRIPTION_LIMIT: usize = 4_096;
const BUTTON_LABEL_LIMIT: usize = 80;
const APPROVE: &str = "approval:once";
const APPROVE_RUN: &str = "approval:run";
const DENY: &str = "approval:deny";

impl Run {
    pub async fn approve(&self, tool: &str, action: &str) -> Result<bool> {
        if self
            .grants
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .contains(tool)
        {
            return Ok(true);
        }

        let buttons = vec![serenity::CreateActionRow::Buttons(vec![
            serenity::CreateButton::new(APPROVE)
                .label("Approve")
                .style(serenity::ButtonStyle::Success),
            serenity::CreateButton::new(APPROVE_RUN)
                .label(truncate_chars(
                    &format!("Approve {tool} for this run"),
                    BUTTON_LABEL_LIMIT,
                ))
                .style(serenity::ButtonStyle::Primary),
            serenity::CreateButton::new(DENY)
                .label("Deny")
                .style(serenity::ButtonStyle::Danger),
        ])];
        let message = self
            .channel_id
            .send_message(
                &self.discord,
                serenity::CreateMessage::new()
                    .embed(approval_embed(action, None))
                    .components(buttons)
                    .allowed_mentions(serenity::CreateAllowedMentions::new()),
            )
            .await
            .context("failed to send approval request")?;
        let mut pending = PendingApproval::new(self, &message, action);
        let collector = serenity::ComponentInteractionCollector::new(&self.discord.shard)
            .message_id(message.id)
            .custom_ids(vec![
                APPROVE.to_owned(),
                APPROVE_RUN.to_owned(),
                DENY.to_owned(),
            ])
            .timeout(APPROVAL_TIMEOUT)
            .stream();
        tokio::pin!(collector);

        let decision = loop {
            let interaction = tokio::select! {
                () = self.cancel.cancelled() => break Decision::Cancelled,
                interaction = collector.next() => match interaction {
                    Some(interaction) => interaction,
                    None => break Decision::TimedOut,
                }
            };
            if interaction.user.id != self.invoker.user.id {
                interaction
                    .create_response(
                        &self.discord,
                        serenity::CreateInteractionResponse::Message(
                            serenity::CreateInteractionResponseMessage::new()
                                .content(format!(
                                    "Only <@{}> can approve this.",
                                    self.invoker.user.id
                                ))
                                .allowed_mentions(serenity::CreateAllowedMentions::new())
                                .ephemeral(true),
                        ),
                    )
                    .await
                    .context("failed to reject approval from another user")?;
                continue;
            }
            interaction
                .defer(&self.discord)
                .await
                .context("failed to acknowledge approval decision")?;
            break match interaction.data.custom_id.as_str() {
                APPROVE => Decision::Approved,
                APPROVE_RUN => Decision::ApprovedForRun,
                _ => Decision::Denied,
            };
        };
        if decision == Decision::ApprovedForRun {
            self.grants
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner())
                .insert(tool.to_owned());
        }
        pending.finish(decision.label(tool)).await?;
        Ok(matches!(
            decision,
            Decision::Approved | Decision::ApprovedForRun
        ))
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Decision {
    Approved,
    ApprovedForRun,
    Denied,
    TimedOut,
    Cancelled,
}

impl Decision {
    fn label(self, tool: &str) -> String {
        match self {
            Self::Approved => "Approved".to_owned(),
            Self::ApprovedForRun => format!("Approved {tool} for this run"),
            Self::Denied => "Denied".to_owned(),
            Self::TimedOut => "Timed out".to_owned(),
            Self::Cancelled => "Cancelled".to_owned(),
        }
    }
}

struct PendingApproval {
    http: Arc<serenity::Http>,
    channel_id: serenity::ChannelId,
    message_id: serenity::MessageId,
    action: String,
    outcome: String,
    armed: bool,
}

impl PendingApproval {
    fn new(run: &Run, message: &serenity::Message, action: &str) -> Self {
        Self {
            http: run.discord.http.clone(),
            channel_id: message.channel_id,
            message_id: message.id,
            action: action.to_owned(),
            outcome: "Cancelled".to_owned(),
            armed: true,
        }
    }

    async fn finish(&mut self, outcome: String) -> Result<()> {
        self.outcome = outcome;
        edit_outcome(
            &self.http,
            self.channel_id,
            self.message_id,
            &self.action,
            &self.outcome,
        )
        .await?;
        self.armed = false;
        Ok(())
    }
}

impl Drop for PendingApproval {
    fn drop(&mut self) {
        if !self.armed {
            return;
        }
        let http = self.http.clone();
        let channel_id = self.channel_id;
        let message_id = self.message_id;
        let action = self.action.clone();
        let outcome = self.outcome.clone();
        tokio::spawn(async move {
            if let Err(error) = edit_outcome(&http, channel_id, message_id, &action, &outcome).await
            {
                warn!(?error, %message_id, "failed to close abandoned approval");
            }
        });
    }
}

async fn edit_outcome(
    http: &serenity::Http,
    channel_id: serenity::ChannelId,
    message_id: serenity::MessageId,
    action: &str,
    outcome: &str,
) -> Result<()> {
    channel_id
        .edit_message(
            http,
            message_id,
            serenity::EditMessage::new()
                .embed(approval_embed(action, Some(outcome)))
                .components(Vec::new()),
        )
        .await
        .context("failed to update approval request")?;
    Ok(())
}

fn approval_embed(action: &str, outcome: Option<&str>) -> serenity::CreateEmbed {
    let description = outcome.map_or_else(
        || truncate_chars(action, EMBED_DESCRIPTION_LIMIT),
        |outcome| {
            let action_limit = EMBED_DESCRIPTION_LIMIT.saturating_sub(outcome.chars().count() + 2);
            format!("{}\n\n{outcome}", truncate_chars(action, action_limit))
        },
    );
    serenity::CreateEmbed::new()
        .title("Approval needed")
        .description(description)
}
