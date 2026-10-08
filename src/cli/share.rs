use std::sync::Arc;

use anyhow::{Context, Result, bail};
use clap::Parser;

use super::{CommandOutput, filename, parse};
use crate::chat::Run;
use crate::sandbox::SandboxKind;

#[derive(Parser)]
#[command(
    name = "share",
    about = "Create an upload URL and a download link for a file shared from the container"
)]
struct Args {
    /// File name shown in the download link
    name: String,
}

pub fn run(run: &Arc<Run>, args: Vec<String>) -> Result<CommandOutput> {
    let args = match parse::<Args>("share", args) {
        Ok(args) => args,
        Err(output) => return Ok(output),
    };
    if run.sandbox != SandboxKind::Modal {
        bail!("share is only available in the real sandbox");
    }
    let storage = run
        .app
        .storage
        .as_ref()
        .context("file sharing is not configured")?;
    let share = storage.share(run.conversation_id, filename(&args.name)?);
    Ok(CommandOutput::text(serde_json::to_string(&share)?))
}
