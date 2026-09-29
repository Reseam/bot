use std::sync::Arc;

use anyhow::Result;
use clap::{Parser, Subcommand};
use poise::serenity_prelude as serenity;

use super::{CommandOutput, parse};
use crate::chat::Run;

mod act;
mod audit;
mod moderation;
mod poll;
mod read;
mod remind;
mod send;
mod server;

#[derive(Parser)]
#[command(
    name = "discord",
    about = "Read and act on the Discord server with the permissions of the person who asked"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Print channel messages, oldest first
    Messages(read::Messages),
    /// Show one message with its attachments, embeds, and reactions
    Message(read::Message),
    /// Print an attachment's text, or save the original file with --output
    Attachment(read::Attachment),
    /// List the categories, channels, and active threads you can see
    Channels,
    /// Show server details and roles
    Server,
    /// Show a member's profile, roles, and permissions in the current channel
    Member(server::Member),
    /// Search members by username or nickname prefix
    Members(server::Members),
    /// Send text or sandbox files. Other channels ask for approval
    Send(send::Send),
    /// Add a reaction to a message
    React(act::React),
    /// Create a public thread
    Thread(act::Thread),
    /// Post a Discord poll
    Poll(poll::Poll),
    /// Pin a message
    Pin(act::Pin),
    /// Unpin a message
    Unpin(act::Pin),
    /// Reminders for the person who asked
    #[command(subcommand)]
    Remind(remind::Remind),
    /// Read the server audit log (team only)
    Audit(audit::Audit),
    /// Moderation actions. Every action asks for approval
    #[command(subcommand)]
    Mod(moderation::Mod),
}

pub async fn run(
    run: &Arc<Run>,
    args: Vec<String>,
    stdin: String,
    files: &mut crate::sandbox::files::Files,
) -> Result<CommandOutput> {
    let cli = match parse::<Cli>("discord", args) {
        Ok(cli) => cli,
        Err(output) => return Ok(output),
    };
    match cli.command {
        Command::Messages(args) => read::messages(run, args).await,
        Command::Message(args) => read::message(run, args).await,
        Command::Attachment(args) => read::attachment(run, args).await,
        Command::Channels => server::channels(run),
        Command::Server => server::server(run),
        Command::Member(args) => server::member(run, args).await,
        Command::Members(args) => server::members(run, args).await,
        Command::Send(args) => send::send(run, args, stdin, files).await,
        Command::React(args) => act::react(run, args).await,
        Command::Thread(args) => act::thread(run, args).await,
        Command::Poll(args) => poll::poll(run, args).await,
        Command::Pin(args) => act::pin(run, args, true).await,
        Command::Unpin(args) => act::pin(run, args, false).await,
        Command::Remind(command) => remind::run(run, command).await,
        Command::Audit(args) => audit::audit(run, args).await,
        Command::Mod(command) => moderation::run(run, command).await,
    }
}

fn channel(run: &Run, requested: Option<u64>) -> serenity::ChannelId {
    requested.map_or(run.channel_id, serenity::ChannelId::new)
}
