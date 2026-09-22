use anyhow::{Context, Result};
use base64::Engine;
use clap::{Args, Parser, Subcommand};

use super::{CommandOutput, OutputFile, parse};
use crate::sandbox::files::Files;

mod directory;
#[cfg(test)]
mod tests;
mod zip;

const MAX_ENTRIES: usize = 256;
const MAX_BYTES: usize = 64 * 1024 * 1024;
const MAX_ARCHIVE_BYTES: usize = 128 * 1024 * 1024;

#[derive(Parser)]
#[command(
    name = "archive",
    about = "Create, list, and extract ZIP archives in the sandbox"
)]
struct Cli {
    #[command(subcommand)]
    command: Command,
}

#[derive(Subcommand)]
enum Command {
    /// Create a ZIP (the create keyword is optional)
    Create(Create),
    /// List ZIP entries and uncompressed sizes
    List { file: String },
    /// Extract into a new directory; its parent must already exist
    Extract {
        file: String,
        #[arg(long, short, value_name = "DIRECTORY")]
        output: String,
    },
}

#[derive(Args)]
struct Create {
    /// Sandbox path to write (replaces an existing file)
    #[arg(long, short, value_name = "PATH")]
    output: String,
    /// Include directories recursively, preserving their structure
    #[arg(long, short)]
    recursive: bool,
    /// Exclude matching entry paths or basenames (repeatable glob)
    #[arg(long, value_name = "GLOB")]
    exclude: Vec<String>,
    /// Files or directories to include (256 entries, 64 MiB total)
    #[arg(required = true, value_name = "PATH")]
    files: Vec<String>,
}

pub async fn run(mut args: Vec<String>, files: &mut Files) -> Result<CommandOutput> {
    if args.first().is_some_and(|arg| {
        !matches!(
            arg.as_str(),
            "create" | "list" | "extract" | "--help" | "-h"
        )
    }) {
        args.insert(0, "create".to_owned());
    }
    let cli = match parse::<Cli>("archive", args) {
        Ok(cli) => cli,
        Err(output) => return Ok(output),
    };
    match cli.command {
        Command::Create(args) => create(args, files).await,
        Command::List { file } => {
            let bytes = files.read(&file, MAX_ARCHIVE_BYTES).await?;
            let (summary, _) = tokio::task::spawn_blocking(move || zip::read(bytes, false))
                .await
                .context("ZIP listing task failed")??;
            Ok(CommandOutput::text(summary))
        }
        Command::Extract { file, output } => {
            let bytes = files.read(&file, MAX_ARCHIVE_BYTES).await?;
            let (_, entries) = tokio::task::spawn_blocking(move || zip::read(bytes, true))
                .await
                .context("ZIP extraction task failed")??;
            let count = entries.len();
            files.write_tree(output.clone(), entries).await?;
            Ok(CommandOutput::text(format!(
                "Extracted {count} entries to {output}"
            )))
        }
    }
}

async fn create(args: Create, files: &mut Files) -> Result<CommandOutput> {
    let entries = files
        .walk(
            args.files,
            args.output.clone(),
            args.recursive,
            args.exclude,
            MAX_ENTRIES,
        )
        .await?;
    zip::validate_entries(
        entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.directory)),
    )?;
    let mut remaining = MAX_BYTES;
    let mut contents = Vec::with_capacity(entries.len());
    for entry in entries {
        let bytes = if entry.directory {
            None
        } else {
            let bytes = files.read(&entry.path, remaining).await?;
            remaining -= bytes.len();
            Some(bytes)
        };
        contents.push((entry.name, bytes));
    }
    let count = contents.len();
    let (size, encoded) = tokio::task::spawn_blocking(move || {
        let bytes = zip::compress(contents)?;
        Ok::<_, anyhow::Error>((
            bytes.len(),
            base64::engine::general_purpose::STANDARD.encode(bytes),
        ))
    })
    .await
    .context("archive creation task failed")??;
    let mut output = CommandOutput::text(format!(
        "Created {} ({count} entries, {size} bytes)",
        args.output
    ));
    output.file = Some(OutputFile {
        path: args.output,
        base64: encoded,
    });
    Ok(output)
}
