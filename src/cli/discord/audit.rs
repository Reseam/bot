use std::fmt::Write as _;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::Args;
use poise::serenity_prelude as serenity;
use serenity::audit_log::Action;

use crate::chat::Run;
use crate::cli::{CommandOutput, snowflake};

#[derive(Args)]
pub struct Audit {
    /// Only entries by this user ID
    #[arg(long, value_parser = snowflake)]
    user: Option<u64>,
    /// Only this action, as printed in the output (such as Member(BanAdd) or member_ban_add) or its Discord number
    #[arg(long, value_parser = parse_action)]
    action: Option<Action>,
    /// Entries older than this entry ID, to page back
    #[arg(long, value_parser = snowflake)]
    before: Option<u64>,
    #[arg(long, default_value_t = 50, value_parser = clap::value_parser!(u8).range(1..=100))]
    limit: u8,
}

pub async fn audit(run: &Arc<Run>, args: Audit) -> Result<CommandOutput> {
    if !run.team {
        bail!("audit logs are available to the team only");
    }
    let permissions = run
        .discord
        .cache
        .guild(run.guild_id)
        .context("server is not available in the Discord cache")?
        .member_permissions(&run.invoker);
    if !permissions.view_audit_log() {
        bail!("invoker is missing View Audit Log");
    }
    let logs = run
        .guild_id
        .audit_logs(
            &run.discord,
            args.action,
            args.user.map(serenity::UserId::new),
            args.before.map(serenity::AuditLogEntryId::new),
            Some(args.limit),
        )
        .await
        .context("failed to read the audit log")?;
    if logs.entries.is_empty() {
        return Ok(CommandOutput::text("No audit log entries."));
    }
    let mut output = String::new();
    for entry in &logs.entries {
        let actor = logs.users.get(&entry.user_id).map_or_else(
            || entry.user_id.to_string(),
            |user| format!("{} ({})", user.name, user.id),
        );
        write!(
            output,
            "{} {} {:?} by {actor}",
            entry.id,
            entry.id.created_at(),
            entry.action
        )?;
        if let Some(target) = entry.target_id {
            write!(output, " target {target}")?;
        }
        if let Some(reason) = &entry.reason {
            write!(output, " reason: {reason}")?;
        }
        if let Some(options) = &entry.options {
            write!(output, "\n  options: {options:?}")?;
        }
        for change in entry.changes.iter().flatten() {
            write!(output, "\n  change: {change:?}")?;
        }
        output.push('\n');
    }
    if logs.entries.len() == usize::from(args.limit)
        && let Some(oldest) = logs.entries.last()
    {
        write!(output, "More entries: --before {}", oldest.id)?;
    }
    Ok(CommandOutput::text(output))
}

fn parse_action(input: &str) -> Result<Action, String> {
    if let Ok(number) = input.parse::<u8>() {
        return Ok(Action::from_value(number));
    }
    let wanted = normalize(input);
    (0..=u8::MAX)
        .map(Action::from_value)
        .filter(|action| !matches!(action, Action::Unknown(_)))
        .find(|action| normalize(&format!("{action:?}")) == wanted)
        .ok_or_else(|| format!("`{input}` is not an audit log action"))
}

fn normalize(name: &str) -> String {
    name.chars()
        .filter(char::is_ascii_alphanumeric)
        .map(|character| character.to_ascii_lowercase())
        .collect()
}

#[cfg(test)]
mod tests {
    use serenity::audit_log::{Action, MemberAction, MessageAction};

    use super::*;

    #[test]
    fn actions_parse_from_printed_names_discord_names_and_numbers() {
        for input in ["Member(BanAdd)", "MEMBER_BAN_ADD", "member_ban_add", "22"] {
            assert!(matches!(
                parse_action(input),
                Ok(Action::Member(MemberAction::BanAdd))
            ));
        }
        assert!(matches!(
            parse_action("message_bulk_delete"),
            Ok(Action::Message(MessageAction::BulkDelete))
        ));
        assert!(matches!(
            parse_action("guild_update"),
            Ok(Action::GuildUpdate)
        ));
        assert!(parse_action("member_explode").is_err());
    }
}
