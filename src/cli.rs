use std::iter::once;
use std::sync::Arc;

use anyhow::Result;
use clap::Parser;
use serde::{Deserialize, Serialize};

use crate::chat::Run;

mod discord;
mod mcp;
mod repo;

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum BridgeCommand {
    Discord,
    Repo,
    Mcp,
}

#[derive(Default, Serialize)]
pub struct CommandOutput {
    pub stdout: String,
    pub stderr: String,
    pub exit_code: i32,
    pub file: Option<OutputFile>,
}

#[derive(Serialize)]
pub struct OutputFile {
    pub path: String,
    pub base64: String,
}

impl CommandOutput {
    pub fn text(stdout: impl Into<String>) -> Self {
        let mut stdout = stdout.into();
        if !stdout.is_empty() && !stdout.ends_with('\n') {
            stdout.push('\n');
        }
        Self {
            stdout,
            ..Self::default()
        }
    }

    fn failure(stderr: String, exit_code: i32) -> Self {
        Self {
            stderr,
            exit_code,
            ..Self::default()
        }
    }
}

pub async fn run(
    run: &Arc<Run>,
    command: BridgeCommand,
    args: Vec<String>,
    stdin: String,
) -> CommandOutput {
    let result = match command {
        BridgeCommand::Discord => discord::run(run, args, stdin).await,
        BridgeCommand::Repo => repo::run(run, args).await,
        BridgeCommand::Mcp => mcp::run(run, args).await,
    };
    result.unwrap_or_else(|error| CommandOutput::failure(format!("error: {error:#}\n"), 1))
}

fn parse<T: Parser>(name: &str, args: Vec<String>) -> Result<T, CommandOutput> {
    T::try_parse_from(once(name.to_owned()).chain(args)).map_err(|error| {
        let rendered = error.render().to_string();
        if error.use_stderr() {
            CommandOutput::failure(rendered, error.exit_code())
        } else {
            CommandOutput::text(rendered)
        }
    })
}

fn snowflake(value: &str) -> Result<u64, String> {
    value
        .trim_start_matches("<@&")
        .trim_start_matches("<@!")
        .trim_start_matches("<@")
        .trim_start_matches("<#")
        .trim_end_matches('>')
        .parse::<u64>()
        .ok()
        .filter(|id| *id != 0)
        .ok_or_else(|| format!("`{value}` is not a Discord ID"))
}
