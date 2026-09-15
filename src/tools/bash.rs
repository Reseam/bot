use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{Tool, ToolOutput};
use crate::chat::Run;

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 1_800;
const TEAM_DESCRIPTION: &str = "Run a bash script in this conversation's sandbox. It is an emulated shell with its own filesystem, not the bot's machine. Files persist across calls in the conversation and are deleted after 8 hours without use.

Paths: /workspace is the writable working directory. /repos/<host>/<owner>/<name> holds repositories cloned with `repo clone`; edits there are discarded.

Built in: common coreutils, grep, rg, sed, awk, jq, yq, sqlite3, xan (CSV), diff, tar, gzip, find, python3 (standard library only), js-exec (JavaScript), curl, and html-to-markdown. There is no git, package manager, or compiler.

Bridge commands (run `COMMAND --help` for details):
- discord: read messages, attachments, channels, members, and the server; send messages, react, create threads, pin; `discord mod` for moderation.
- repo clone URL [--ref REF] [--history]: clone or update an HTTPS repository.
- mcp list, mcp SERVER TOOL --help, mcp SERVER TOOL key=value: call MCP services such as web search.
- view FILE: attach an image file to this result so you can see it.

curl reaches any public host. Requests to configured forge APIs are authenticated automatically. POST, PUT, PATCH, and DELETE requests, moderation, and messages to other channels ask the invoker for approval.";

const MEMBER_DESCRIPTION: &str = "Run a bash script in this conversation's sandbox. It is an emulated shell with its own filesystem, not the bot's machine. Files persist across calls in the conversation and are deleted after 8 hours without use.

Paths: /workspace is the writable working directory.

Built in: common coreutils, grep, rg, sed, awk, jq, yq, sqlite3, xan (CSV), diff, tar, gzip, and find. There is no network access, python3, js-exec, git, package manager, or compiler.

Bridge commands (run `COMMAND --help` for details):
- discord: read messages, attachments, channels, members, and the server; send messages, react, create threads, pin; `discord mod` for moderation.
- view FILE: attach an image file to this result so you can see it.

Moderation and messages to other channels ask the invoker for approval.";

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Arguments {
    command: String,
    timeout_secs: Option<u64>,
}

pub fn tool(run: &Arc<Run>) -> Tool {
    let description = if run.team {
        TEAM_DESCRIPTION
    } else {
        MEMBER_DESCRIPTION
    };
    Tool::new("bash", description, run.clone(), bash)
}

async fn bash(run: Arc<Run>, arguments: Arguments) -> Result<ToolOutput> {
    let timeout = arguments.timeout_secs.unwrap_or(DEFAULT_TIMEOUT_SECS);
    if !(1..=MAX_TIMEOUT_SECS).contains(&timeout) {
        bail!("timeout_secs must be between 1 and {MAX_TIMEOUT_SECS}");
    }
    let execution = run
        .app
        .sandbox
        .exec(&run, &arguments.command, Duration::from_secs(timeout))
        .await?;
    Ok(execution.into_output())
}
