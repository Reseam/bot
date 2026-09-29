use std::sync::Arc;
use std::time::Duration;

use anyhow::{Result, bail};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{Tool, ToolOutput};
use crate::chat::Run;

const DEFAULT_TIMEOUT_SECS: u64 = 120;
const MAX_TIMEOUT_SECS: u64 = 1_800;
const TEAM_DESCRIPTION: &str = "Run a bash script in this conversation's sandbox. It is an emulated shell with its own filesystem, not the bot's machine. Files persist across calls in the conversation and are deleted after 8 hours without use. Keep binary data in files: printed binary output is replaced with its byte count.

Paths: /workspace is the writable working directory. /repos/<host>/<owner>/<name> holds repositories cloned with `repo clone`; edits there are discarded.

Built in: common coreutils, grep, rg, sed, awk, jq, yq, sqlite3, xan (CSV), diff, tar, gzip, find, python3 (standard library only), js-exec (JavaScript), curl, and html-to-markdown. There is no git, package manager, or compiler.

Bridge commands (run `COMMAND --help` for details):
- discord: read messages (filter with --author and --contains), attachments, channels, members, and the server; send messages, react, create threads, pin; `discord poll` posts a real Discord poll; `discord remind set|list|cancel` manages the invoker's reminders; `discord audit` reads the server audit log (filter with --user and --action); `discord mod` for moderation.
- discord send --file PATH [--filename NAME] [TEXT]: upload a sandbox file as an attachment to this Discord channel. Repeat --file for multiple files. Creating a file for the user includes sending it with this command.
- archive --output bundle.zip [--recursive] PATH...: create a ZIP in the sandbox. Then use discord send --file bundle.zip to deliver it. archive list FILE inspects a ZIP; archive extract FILE --output NEW_DIRECTORY extracts one.
- repo clone URL [--ref REF] [--history]: clone or update an HTTPS repository.
- mcp list, mcp SERVER TOOL --help, mcp SERVER TOOL key=value: call MCP services such as web search.
- upload [--form FIELD] [--method M] [--header 'K: V'] URL FILE: send a file's exact bytes, as a multipart part with --form or as the raw body. Use it for images, video, and any other binary upload; curl only sends text bodies.
- view FILE: attach an image file to this result so you can see it.

curl and upload reach any public host. Requests to configured forge APIs are authenticated automatically. POST, PUT, PATCH, and DELETE requests, moderation, and messages or polls in other channels ask the invoker for approval.";

const MEMBER_DESCRIPTION: &str = "Run a bash script in this conversation's sandbox. It is an emulated shell with its own filesystem, not the bot's machine. Files persist across calls in the conversation and are deleted after 8 hours without use. Keep binary data in files: printed binary output is replaced with its byte count.

Paths: /workspace is the writable working directory.

Built in: common coreutils, grep, rg, sed, awk, jq, yq, sqlite3, xan (CSV), diff, tar, gzip, and find. There is no network access, python3, js-exec, git, package manager, or compiler.

Bridge commands (run `COMMAND --help` for details):
- discord: read messages (filter with --author and --contains), attachments, channels, members, and the server; send messages, react, create threads, pin; `discord poll` posts a real Discord poll; `discord remind set|list|cancel` manages the invoker's reminders; `discord mod` for moderation.
- discord send --file PATH [--filename NAME] [TEXT]: upload a sandbox file as an attachment to this Discord channel. Repeat --file for multiple files. Creating a file for the user includes sending it with this command.
- archive --output bundle.zip [--recursive] PATH...: create a ZIP in the sandbox. Then use discord send --file bundle.zip to deliver it. archive list FILE inspects a ZIP; archive extract FILE --output NEW_DIRECTORY extracts one.
- view FILE: attach an image file to this result so you can see it.

Moderation and messages or polls in other channels ask the invoker for approval.";

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
    let output = run
        .app
        .sandbox
        .exec(&run, &arguments.command, Duration::from_secs(timeout))
        .await?
        .into_output();
    let notices = run.take_approval_notices();
    if notices.is_empty() {
        return Ok(output);
    }
    Ok(ToolOutput {
        text: format!("{}\n{}", notices.join("\n"), output.text),
        images: output.images,
    })
}
