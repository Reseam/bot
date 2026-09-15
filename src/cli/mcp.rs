use std::fmt::Write as _;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value};

use super::CommandOutput;
use crate::chat::Run;
use crate::text::truncate_chars;

const APPROVAL_ARGUMENTS_LIMIT: usize = 1_500;
const USAGE: &str = "Usage:
  mcp list                                   List servers and their tools
  mcp SERVER TOOL --help                     Show a tool's description and input schema
  mcp SERVER TOOL [key=value ...] [--json OBJECT]

Values are parsed as JSON when valid (numbers, booleans, arrays, objects) and used as strings otherwise.
";

pub async fn run(run: &Arc<Run>, args: Vec<String>) -> Result<CommandOutput> {
    let (server, tool, rest) = match args.as_slice() {
        [command] if command == "list" => return list(run),
        [server, tool, rest @ ..] if !server.starts_with('-') => (server, tool, rest),
        _ => return Ok(CommandOutput::text(USAGE)),
    };
    let exposed = run
        .app
        .mcp
        .tool(server, tool)
        .with_context(|| format!("unknown MCP tool `{server} {tool}`; run `mcp list`"))?;
    if rest.iter().any(|arg| arg == "--help" || arg == "-h") {
        let schema = serde_json::to_string_pretty(&exposed.input_schema)
            .context("failed to format input schema")?;
        return Ok(CommandOutput::text(format!(
            "{}\n\nInput schema:\n{schema}",
            exposed.description
        )));
    }
    let arguments = Value::Object(parse_arguments(rest)?);
    if exposed.approve {
        let preview =
            serde_json::to_string_pretty(&arguments).context("failed to format arguments")?;
        run.approve(
            &format!("mcp {server} {tool}"),
            &format!(
                "Call {server} {tool}\n```json\n{}\n```",
                truncate_chars(&preview, APPROVAL_ARGUMENTS_LIMIT)
            ),
        )
        .await?;
    }
    let output = run
        .app
        .mcp
        .call(server, tool, arguments, exposed.timeout)
        .await?;
    let mut text = output.text;
    if !output.images.is_empty() {
        text.push_str(&format!("\n[{} images omitted]", output.images.len()));
    }
    Ok(CommandOutput::text(text))
}

fn list(run: &Arc<Run>) -> Result<CommandOutput> {
    let mut output = String::new();
    for status in run.app.mcp.status() {
        match status.error {
            Some(error) => writeln!(output, "{}: unavailable ({error})", status.name)?,
            None => writeln!(output, "{}:", status.name)?,
        }
        for tool in run
            .app
            .mcp
            .tools()
            .iter()
            .filter(|tool| tool.server == status.name)
        {
            let summary = tool.description.lines().next().unwrap_or_default();
            writeln!(output, "  {}: {summary}", tool.name)?;
        }
    }
    Ok(CommandOutput::text(if output.is_empty() {
        "No MCP servers are configured.".to_owned()
    } else {
        output
    }))
}

fn parse_arguments(args: &[String]) -> Result<Map<String, Value>> {
    let mut arguments = Map::new();
    let mut args = args.iter();
    while let Some(arg) = args.next() {
        if arg == "--json" {
            let json = args.next().context("--json needs a JSON object")?;
            let Value::Object(object) =
                serde_json::from_str(json).context("--json is not valid JSON")?
            else {
                bail!("--json must be a JSON object");
            };
            arguments.extend(object);
        } else {
            let (key, value) = arg
                .split_once('=')
                .with_context(|| format!("expected key=value, got `{arg}`"))?;
            let value =
                serde_json::from_str(value).unwrap_or_else(|_| Value::String(value.to_owned()));
            arguments.insert(key.to_owned(), value);
        }
    }
    Ok(arguments)
}
