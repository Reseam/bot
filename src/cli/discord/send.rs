use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::Args;
use poise::serenity_prelude as serenity;

use super::channel;
use crate::chat::Run;
use crate::cli::{CommandOutput, filename, snowflake};
use crate::discord::{jump_link, resolve_channel, send_permission};
use crate::text::{DISCORD_MESSAGE_LIMIT, truncate_chars};

const MAX_ATTACHMENTS: usize = 10;
const MAX_UPLOAD_BYTES: usize = 10 * 1024 * 1024;
const APPROVAL_PREVIEW_LIMIT: usize = 1_500;

#[derive(Args)]
pub struct Send {
    /// Channel or thread ID (default: the current channel)
    #[arg(long, value_parser = snowflake)]
    channel: Option<u64>,
    /// Reply to this message ID
    #[arg(long, value_parser = snowflake)]
    reply: Option<u64>,
    /// Attach a sandbox file (repeat for multiple files; 10 MiB total)
    #[arg(long = "file", value_name = "PATH")]
    files: Vec<String>,
    /// Custom names in --file order (provide one per file when used)
    #[arg(long = "filename", value_name = "NAME", requires = "files")]
    filenames: Vec<String>,
    /// Message text. Read from stdin when omitted
    text: Option<String>,
}

pub async fn send(
    run: &Arc<Run>,
    args: Send,
    stdin: String,
    files: &mut crate::sandbox::files::Files,
) -> Result<CommandOutput> {
    let text = args.text.unwrap_or(stdin);
    let content = text.trim_end_matches('\n');
    validate_send(content, &args.files)?;
    let names = attachment_names(&args.files, &args.filenames)?;
    let channel_id = channel(run, args.channel);
    let access = resolve_channel(&run.discord, run.guild_id, &run.invoker, channel_id).await?;
    let mut required = send_permission(&access.channel) | serenity::Permissions::VIEW_CHANNEL;
    if !args.files.is_empty() {
        required |= serenity::Permissions::ATTACH_FILES;
    }
    if !access.permissions.contains(required) {
        bail!("invoker cannot send messages in #{}", access.channel.name);
    }
    let mut attachments = Vec::with_capacity(args.files.len());
    let mut remaining = MAX_UPLOAD_BYTES;
    for (path, name) in args.files.iter().zip(names) {
        let bytes = files.read(path, remaining).await?;
        remaining -= bytes.len();
        attachments.push(serenity::CreateAttachment::bytes(bytes, name));
    }
    let mut preview = truncate_chars(content, APPROVAL_PREVIEW_LIMIT);
    for attachment in &attachments {
        preview.push_str(&format!(
            "\nFile: {} ({} bytes)",
            attachment.filename,
            attachment.data.len()
        ));
    }
    if channel_id != run.channel_id {
        run.approve(
            &format!("discord send {channel_id}"),
            &format!("Send to <#{channel_id}>:\n>>> {}", preview),
        )
        .await?;
    }
    let mut builder = serenity::CreateMessage::new()
        .content(content)
        .add_files(attachments)
        .allowed_mentions(serenity::CreateAllowedMentions::new());
    if let Some(reply) = args.reply {
        builder = builder.reference_message((channel_id, serenity::MessageId::new(reply)));
    }
    let message = channel_id
        .send_message(&run.discord, builder)
        .await
        .context("failed to send Discord message")?;
    Ok(CommandOutput::text(format!(
        "Sent {}",
        jump_link(run.guild_id, channel_id, message.id)
    )))
}

fn attachment_names(paths: &[String], names: &[String]) -> Result<Vec<String>> {
    if !names.is_empty() && names.len() != paths.len() {
        bail!("provide one --filename for each --file, in the same order");
    }
    let names = if names.is_empty() {
        paths
            .iter()
            .map(|path| path.rsplit('/').next().unwrap_or_default())
            .collect::<Vec<_>>()
    } else {
        names.iter().map(String::as_str).collect()
    };
    names
        .into_iter()
        .map(|name| filename(name).map(str::to_owned))
        .collect()
}

fn validate_send(content: &str, files: &[String]) -> Result<()> {
    if content.is_empty() && files.is_empty() {
        bail!("provide message text or at least one --file");
    }
    if content.chars().count() > DISCORD_MESSAGE_LIMIT {
        bail!("message must be at most {DISCORD_MESSAGE_LIMIT} characters");
    }
    if files.len() > MAX_ATTACHMENTS {
        bail!("a message can have at most {MAX_ATTACHMENTS} files");
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use clap::Parser;

    use super::{attachment_names, validate_send};
    use crate::cli::discord::{Cli, Command};

    #[test]
    fn filename_options_preserve_file_order() {
        let cli = Cli::try_parse_from([
            "discord",
            "send",
            "--file",
            "/tmp/a",
            "--filename",
            "report.csv",
            "--file",
            "/tmp/b",
            "--filename",
            "chart.png",
            "Results",
        ])
        .unwrap();
        let Command::Send(args) = cli.command else {
            panic!("expected send")
        };
        assert_eq!(args.text.as_deref(), Some("Results"));
        assert_eq!(args.files, ["/tmp/a", "/tmp/b"]);
        assert_eq!(
            attachment_names(&args.files, &args.filenames).unwrap(),
            ["report.csv", "chart.png"]
        );
        assert!(Cli::try_parse_from(["discord", "send", "--filename", "report.csv"]).is_err());
    }

    #[test]
    fn custom_names_match_files_and_cannot_be_paths() {
        let paths = vec!["/tmp/a.csv".into(), "/workspace/b.png".into()];
        assert_eq!(attachment_names(&paths, &[]).unwrap(), ["a.csv", "b.png"]);
        assert_eq!(
            attachment_names(&paths, &["report.csv".into(), "chart.png".into()]).unwrap(),
            ["report.csv", "chart.png"]
        );
        assert!(attachment_names(&paths, &["report.csv".into()]).is_err());
        for name in [
            "",
            ".",
            "..",
            "../secret",
            "dir/file",
            "dir\\file",
            "a\nb",
            "C:secret",
        ] {
            assert!(attachment_names(&["source".into()], &[name.into()]).is_err());
        }
    }

    #[test]
    fn attachment_only_messages_and_discord_limits() {
        assert!(validate_send("", &[]).is_err());
        assert!(validate_send("", &["report.pdf".into()]).is_ok());
        assert!(validate_send(&"🦀".repeat(2000), &[]).is_ok());
        assert!(validate_send(&"🦀".repeat(2001), &["report.pdf".into()]).is_err());
        assert!(validate_send("", &vec!["file".into(); 10]).is_ok());
        assert!(validate_send("", &vec!["file".into(); 11]).is_err());
    }
}
