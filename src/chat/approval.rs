use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use futures::StreamExt;
use poise::serenity_prelude as serenity;
use tracing::warn;

use super::Run;
use crate::text::truncate_chars;

const APPROVAL_TIMEOUT: Duration = Duration::from_secs(5 * 60);
const EMBED_DESCRIPTION_LIMIT: usize = 4_096;
const APPROVE: &str = "approval:once";
const APPROVE_RUN: &str = "approval:run";
const DENY: &str = "approval:deny";

impl Run {
    pub async fn approve(&self, scope: &str, action: &str) -> Result<()> {
        if self.grants.lock().contains(scope) {
            return Ok(());
        }

        let buttons = vec![serenity::CreateActionRow::Buttons(vec![
            serenity::CreateButton::new(APPROVE)
                .label("Approve")
                .style(serenity::ButtonStyle::Success),
            serenity::CreateButton::new(APPROVE_RUN)
                .label("Approve for this run")
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
                    .embed(
                        serenity::CreateEmbed::new()
                            .title("Approval needed")
                            .description(truncate_chars(action, EMBED_DESCRIPTION_LIMIT))
                            .footer(serenity::CreateEmbedFooter::new(format!(
                                "Approve for this run covers: {scope}"
                            ))),
                    )
                    .components(buttons)
                    .allowed_mentions(serenity::CreateAllowedMentions::new()),
            )
            .await
            .context("failed to send approval request")?;
        let _message = ApprovalMessage {
            http: self.discord.http.clone(),
            channel_id: message.channel_id,
            message_id: message.id,
        };
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
            self.grants.lock().insert(scope.to_owned());
        }
        match decision {
            Decision::Approved | Decision::ApprovedForRun => Ok(()),
            Decision::Denied => bail!("the invoker denied this action; do not try it again"),
            Decision::TimedOut => bail!("the approval request timed out"),
            Decision::Cancelled => bail!("the run was cancelled"),
        }
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

struct ApprovalMessage {
    http: Arc<serenity::Http>,
    channel_id: serenity::ChannelId,
    message_id: serenity::MessageId,
}

impl Drop for ApprovalMessage {
    fn drop(&mut self) {
        let http = self.http.clone();
        let channel_id = self.channel_id;
        let message_id = self.message_id;
        tokio::spawn(async move {
            if let Err(error) = channel_id.delete_message(&http, message_id).await {
                warn!(?error, %message_id, "failed to delete approval request");
            }
        });
    }
}
