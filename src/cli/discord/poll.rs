use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use clap::Args;
use poise::serenity_prelude as serenity;

use super::channel;
use crate::chat::Run;
use crate::cli::{CommandOutput, snowflake};
use crate::discord::{jump_link, resolve_channel, send_permission};

const MAX_QUESTION_CHARS: usize = 300;
const MAX_ANSWER_CHARS: usize = 55;
const MAX_ANSWERS: usize = 10;

#[derive(Args)]
pub struct Poll {
    /// Channel or thread ID (default: the current channel)
    #[arg(long, value_parser = snowflake)]
    channel: Option<u64>,
    /// How long voting stays open, 1 to 768 hours
    #[arg(long, default_value_t = 24, value_parser = clap::value_parser!(u16).range(1..=768))]
    hours: u16,
    /// Let voters pick more than one answer
    #[arg(long)]
    multiple: bool,
    /// Up to 300 characters
    question: String,
    /// 2 to 10 answers, up to 55 characters each
    #[arg(required = true)]
    answers: Vec<String>,
}

pub async fn poll(run: &Arc<Run>, args: Poll) -> Result<CommandOutput> {
    validate(&args)?;
    let channel_id = channel(run, args.channel);
    let access = resolve_channel(&run.discord, run.guild_id, &run.invoker, channel_id).await?;
    let required = send_permission(&access.channel)
        | serenity::Permissions::VIEW_CHANNEL
        | serenity::Permissions::SEND_POLLS;
    if !access.permissions.contains(required) {
        bail!("invoker cannot send polls in #{}", access.channel.name);
    }
    if channel_id != run.channel_id {
        let answers = args
            .answers
            .iter()
            .map(|answer| format!("- {answer}"))
            .collect::<Vec<_>>()
            .join("\n");
        run.approve(
            &format!("discord poll {channel_id}"),
            &format!(
                "Post a poll in <#{channel_id}>:\n>>> **{}**\n{answers}",
                args.question
            ),
        )
        .await?;
    }
    let answers = args
        .answers
        .iter()
        .map(|answer| serenity::CreatePollAnswer::new().text(answer))
        .collect();
    let mut poll = serenity::CreatePoll::new()
        .question(&args.question)
        .answers(answers)
        .duration(Duration::from_secs(u64::from(args.hours) * 3_600));
    if args.multiple {
        poll = poll.allow_multiselect();
    }
    let message = channel_id
        .send_message(&run.discord, serenity::CreateMessage::new().poll(poll))
        .await
        .context("failed to send Discord poll")?;
    if channel_id == run.channel_id {
        run.app.runs.register_message(message.id, run);
    }
    Ok(CommandOutput::text(format!(
        "Posted poll {}",
        jump_link(run.guild_id, channel_id, message.id)
    )))
}

fn validate(args: &Poll) -> Result<()> {
    if !(1..=MAX_QUESTION_CHARS).contains(&args.question.trim().chars().count()) {
        bail!("the question must be 1 to {MAX_QUESTION_CHARS} characters");
    }
    if !(2..=MAX_ANSWERS).contains(&args.answers.len()) {
        bail!("a poll needs 2 to {MAX_ANSWERS} answers");
    }
    if args
        .answers
        .iter()
        .any(|answer| !(1..=MAX_ANSWER_CHARS).contains(&answer.trim().chars().count()))
    {
        bail!("each answer must be 1 to {MAX_ANSWER_CHARS} characters");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::validate;
    use crate::cli::discord::{Cli, Command};

    fn parse(args: &[&str]) -> anyhow::Result<super::Poll> {
        let cli = Cli::try_parse_from(["discord", "poll"].iter().chain(args))?;
        let Command::Poll(poll) = cli.command else {
            panic!("expected poll")
        };
        Ok(poll)
    }

    #[test]
    fn poll_limits_follow_discord() -> anyhow::Result<()> {
        let poll = parse(&["--hours", "48", "--multiple", "Lunch?", "Pizza", "Sushi"])?;
        assert_eq!((poll.hours, poll.multiple), (48, true));
        assert_eq!(poll.answers, ["Pizza", "Sushi"]);
        assert!(validate(&poll).is_ok());

        assert_eq!(parse(&["Lunch?", "Pizza", "Sushi"])?.hours, 24);
        assert!(parse(&["--hours", "0", "Lunch?", "Pizza", "Sushi"]).is_err());
        assert!(parse(&["--hours", "769", "Lunch?", "Pizza", "Sushi"]).is_err());
        assert!(parse(&["Lunch?"]).is_err());

        assert!(validate(&parse(&["Lunch?", "Pizza"])?).is_err());
        let eleven = ["Q"].into_iter().chain(["a"; 11]).collect::<Vec<_>>();
        assert!(validate(&parse(&eleven)?).is_err());
        let long_answer = "a".repeat(56);
        assert!(validate(&parse(&["Lunch?", "Pizza", &long_answer])?).is_err());
        assert!(validate(&parse(&["Lunch?", "Pizza", " "])?).is_err());
        let long_question = "q".repeat(301);
        assert!(validate(&parse(&[&long_question, "Pizza", "Sushi"])?).is_err());
        Ok(())
    }
}
