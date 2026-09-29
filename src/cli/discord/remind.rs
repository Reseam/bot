use std::sync::Arc;

use anyhow::{Context, Result};
use clap::Subcommand;
use poise::serenity_prelude as serenity;

use crate::chat::Run;
use crate::cli::CommandOutput;
use crate::reminders::{self, NewReminder};

#[derive(Subcommand)]
pub enum Remind {
    /// Remind the person who asked, in this channel
    Set {
        /// Such as 30m, 2h 30m, or 3d (at most 365 days)
        delay: String,
        /// What to remind them about, up to 1000 characters
        message: String,
    },
    /// List the reminders of the person who asked
    List,
    /// Cancel one of their reminders by ID
    Cancel { id: i64 },
}

pub async fn run(run: &Arc<Run>, command: Remind) -> Result<CommandOutput> {
    let db = &run.app.db;
    let user_id = run.invoker.user.id;
    match command {
        Remind::Set { delay, message } => {
            let due_at = reminders::schedule(
                db,
                &NewReminder {
                    guild_id: run.guild_id,
                    channel_id: run.channel_id,
                    user_id,
                    message: &message,
                },
                &delay,
            )
            .await?;
            Ok(CommandOutput::text(format!(
                "Reminder set for {} (<t:{due_at}:R>). The bot will ping <@{user_id}> in this channel.",
                timestamp(due_at)?
            )))
        }
        Remind::List => {
            let pending = reminders::list(db, run.guild_id, user_id).await?;
            if pending.is_empty() {
                return Ok(CommandOutput::text("No reminders."));
            }
            let lines = pending
                .iter()
                .map(|reminder| {
                    Ok(format!(
                        "{} {} <#{}> {}",
                        reminder.id,
                        timestamp(reminder.due_at)?,
                        reminder.channel_id()?,
                        reminder.message
                    ))
                })
                .collect::<Result<Vec<_>>>()?;
            Ok(CommandOutput::text(lines.join("\n")))
        }
        Remind::Cancel { id } => {
            let text = if reminders::cancel(db, run.guild_id, user_id, id).await? {
                format!("Cancelled reminder {id}.")
            } else {
                format!("<@{user_id}> has no reminder {id}.")
            };
            Ok(CommandOutput::text(text))
        }
    }
}

fn timestamp(unix: i64) -> Result<serenity::Timestamp> {
    serenity::Timestamp::from_unix_timestamp(unix).context("reminder time is out of range")
}
