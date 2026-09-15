use std::path::Path;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use clap::{Parser, Subcommand};
use reqwest::Url;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use super::{CommandOutput, parse};
use crate::chat::Run;
use crate::sandbox::{repos_dir, touch};

const GIT_TIMEOUT: Duration = Duration::from_secs(10 * 60);

#[derive(Parser)]
#[command(name = "repo", about = "Clone repositories into /repos")]
struct Cli {
    #[command(subcommand)]
    command: RepoCommand,
}

#[derive(Subcommand)]
enum RepoCommand {
    /// Clone an HTTPS repository, or update it if it is already cloned
    Clone {
        /// Repository URL, such as https://github.com/owner/name
        url: String,
        /// Branch or tag (default: the default branch)
        #[arg(long = "ref")]
        reference: Option<String>,
        /// Fetch the full history instead of only the latest commit
        #[arg(long)]
        history: bool,
    },
}

pub async fn run(run: &Arc<Run>, args: Vec<String>) -> Result<CommandOutput> {
    let cli = match parse::<Cli>("repo", args) {
        Ok(cli) => cli,
        Err(output) => return Ok(output),
    };
    let RepoCommand::Clone {
        url,
        reference,
        history,
    } = cli.command;
    let repository = Repository::parse(&url)?;
    let root = repos_dir(&run.app.config.data_dir)
        .join(&repository.host)
        .join(&repository.owner)
        .join(&repository.name);
    let _lock = run.app.repo_locks.lock(&root).await;
    let authorization = run
        .app
        .forges
        .iter()
        .find_map(|forge| forge.git_authorization(&repository.host));
    let git = Git {
        authorization: authorization.as_deref(),
        cancel: &run.cancel,
    };
    let reference_arg = reference.as_deref().unwrap_or("HEAD");
    if root.join(".git").exists() {
        let mut fetch = vec!["fetch", "origin"];
        if !history {
            fetch.extend(["--depth", "1"]);
        } else if root.join(".git/shallow").exists() {
            fetch.push("--unshallow");
        }
        fetch.extend(["--", reference_arg]);
        git.run(&root, &fetch).await?;
        git.run(&root, &["checkout", "--force", "FETCH_HEAD"])
            .await?;
    } else {
        let parent = root.parent().context("repository path has no parent")?;
        tokio::fs::create_dir_all(parent)
            .await
            .context("failed to create repository directory")?;
        let destination = root.to_str().context("repository path is not UTF-8")?;
        let mut clone = vec!["clone"];
        if !history {
            clone.extend(["--depth", "1"]);
        }
        if let Some(reference) = &reference {
            clone.extend(["--branch", reference.as_str()]);
        }
        clone.extend(["--", repository.url.as_str(), destination]);
        git.run(parent, &clone).await?;
    }
    let commit = git.run(&root, &["rev-parse", "HEAD"]).await?;
    let marked = root.clone();
    tokio::task::spawn_blocking(move || touch(&marked))
        .await
        .context("repository bookkeeping task failed")??;
    Ok(CommandOutput::text(format!(
        "Ready at /repos/{}/{}/{} (commit {})",
        repository.host,
        repository.owner,
        repository.name,
        commit.trim()
    )))
}

#[derive(Debug, PartialEq, Eq)]
struct Repository {
    url: String,
    host: String,
    owner: String,
    name: String,
}

impl Repository {
    fn parse(value: &str) -> Result<Self> {
        let url = Url::parse(value).context("invalid repository URL")?;
        if url.scheme() != "https" || !url.username().is_empty() || url.password().is_some() {
            bail!("repository URL must use https without credentials");
        }
        if url.port_or_known_default() != Some(443) {
            bail!("repository URL must use the standard HTTPS port");
        }
        if url.query().is_some() || url.fragment().is_some() {
            bail!("repository URL must not have a query or fragment");
        }
        let host = url.host_str().context("repository URL has no host")?;
        let parts = url
            .path_segments()
            .context("repository URL has no path")?
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        let [owner, name] = parts.as_slice() else {
            bail!("repository URL path must be owner/name");
        };
        let name = name.strip_suffix(".git").unwrap_or(name);
        if [*owner, name]
            .iter()
            .any(|part| part.is_empty() || part.starts_with('.') || part.contains('%'))
        {
            bail!("repository owner and name must be plain path segments");
        }
        Ok(Self {
            url: value.to_owned(),
            host: host.to_owned(),
            owner: (*owner).to_owned(),
            name: name.to_owned(),
        })
    }
}

struct Git<'a> {
    authorization: Option<&'a str>,
    cancel: &'a CancellationToken,
}

impl Git<'_> {
    async fn run(&self, cwd: &Path, args: &[&str]) -> Result<String> {
        let mut command = Command::new("git");
        if let Some(authorization) = self.authorization {
            command.args([
                "-c",
                &format!("http.extraHeader=Authorization: {authorization}"),
            ]);
        }
        command
            .args(args)
            .current_dir(cwd)
            .env("GIT_TERMINAL_PROMPT", "0")
            .kill_on_drop(true);
        let output = tokio::select! {
            () = self.cancel.cancelled() => bail!("git was cancelled"),
            result = tokio::time::timeout(GIT_TIMEOUT, command.output()) => {
                result.map_err(|_| anyhow!("git timed out"))?.context("failed to run git")?
            }
        };
        if !output.status.success() {
            bail!(
                "git failed: {}",
                String::from_utf8_lossy(&output.stderr).trim()
            );
        }
        Ok(String::from_utf8_lossy(&output.stdout).into_owned())
    }
}

#[cfg(test)]
mod tests;
