use std::collections::{BTreeMap, HashMap};
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard, Weak};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use reqwest::Url;
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use super::list;
use crate::chat::Run;
use crate::config::ForgeKind;
use crate::forge::Forge;
use crate::tools::{Tool, ToolOutput};

const GIT_TIMEOUT: Duration = Duration::from_secs(5 * 60);

#[derive(Default)]
pub struct RepoLocks(Mutex<HashMap<PathBuf, Weak<tokio::sync::Mutex<()>>>>);

impl RepoLocks {
    fn get(&self, path: &Path) -> Arc<tokio::sync::Mutex<()>> {
        let mut locks = lock(&self.0);
        locks.retain(|_, lock| lock.strong_count() > 0);
        locks.get(path).and_then(Weak::upgrade).unwrap_or_else(|| {
            let entry = Arc::new(tokio::sync::Mutex::new(()));
            locks.insert(path.to_owned(), Arc::downgrade(&entry));
            entry
        })
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

pub(super) fn tool(run: &Arc<Run>) -> Tool {
    Tool::new::<CloneRepo, _, _, _>(
        "repo_clone",
        "Clone an HTTPS git repository or update its existing working copy.",
        run.clone(),
        clone_repo,
    )
}

#[derive(Debug, Eq, PartialEq)]
pub(super) struct RepoUrl {
    url: String,
    host: String,
    owner: String,
    name: String,
}

impl RepoUrl {
    pub(super) fn parse(value: &str) -> Result<Self> {
        let url = Url::parse(value).context("invalid repository URL")?;
        if url.scheme() != "https" || url.username() != "" || url.password().is_some() {
            bail!("repository URL must use https without credentials")
        }
        if url.port_or_known_default() != Some(443) {
            bail!("repository URL must use the standard HTTPS port")
        }
        if url.query().is_some() || url.fragment().is_some() {
            bail!("repository URL must not have a query or fragment")
        }
        let host = url.host_str().context("repository URL has no host")?;
        let parts = url
            .path_segments()
            .context("repository URL cannot be a base")?
            .filter(|part| !part.is_empty())
            .collect::<Vec<_>>();
        if parts.len() != 2
            || parts.iter().any(|part| {
                *part == ".."
                    || *part == "."
                    || part.eq_ignore_ascii_case("%2e")
                    || part.eq_ignore_ascii_case("%2e%2e")
            })
        {
            bail!("repository URL path must be owner/name")
        }
        let name = parts[1].strip_suffix(".git").unwrap_or(parts[1]);
        if name.is_empty() {
            bail!("repository name must not be empty")
        }
        Ok(Self {
            url: value.to_owned(),
            host: host.to_owned(),
            owner: parts[0].to_owned(),
            name: name.to_owned(),
        })
    }

    pub(super) fn id(&self) -> String {
        format!("{}/{}/{}", self.host, self.owner, self.name)
    }
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CloneRepo {
    url: String,
    #[serde(rename = "ref")]
    reference: Option<String>,
}

async fn clone_repo(run: Arc<Run>, args: CloneRepo) -> Result<ToolOutput> {
    let result = clone_checkout(
        &run.app.config.data_dir,
        &run.app.repo_locks,
        &run.app.forges,
        &run.cancel,
        &args.url,
        args.reference.as_deref(),
    )
    .await?;
    tracing::debug!(path = %result.root.display(), "repository working copy ready");
    Ok(ToolOutput::text(format!(
        "Repository: {}\nCommit: {}\n\n{}",
        result.id,
        result.commit.trim(),
        result.listing
    )))
}

pub(super) struct CloneResult {
    pub id: String,
    pub commit: String,
    pub listing: String,
    pub root: PathBuf,
}

pub(super) async fn clone_checkout(
    data_dir: &Path,
    locks: &RepoLocks,
    forges: &BTreeMap<String, Forge>,
    cancel: &CancellationToken,
    url: &str,
    reference: Option<&str>,
) -> Result<CloneResult> {
    let parsed = RepoUrl::parse(url)?;
    let root = data_dir
        .join("repos")
        .join(&parsed.host)
        .join(&parsed.owner)
        .join(&parsed.name);
    let repo_lock = locks.get(&root);
    let _guard = repo_lock.lock().await;
    tokio::fs::create_dir_all(root.parent().context("repository path has no parent")?)
        .await
        .context("failed to create repository directory")?;
    let auth = git_auth(forges, &parsed.host);
    let fetch_reference = reference.unwrap_or("HEAD");
    if root.exists() {
        run_git(
            cancel,
            &root,
            auth.as_deref(),
            &["fetch", "--depth", "1", "origin", "--", fetch_reference],
        )
        .await?;
        run_git(
            cancel,
            &root,
            auth.as_deref(),
            &["checkout", "--force", "FETCH_HEAD"],
        )
        .await?;
    } else {
        let mut arguments = vec!["clone", "--depth", "1"];
        if let Some(reference) = reference {
            arguments.extend(["--branch", reference]);
        }
        arguments.extend([parsed.url.as_str(), path_text(&root)?]);
        run_git(cancel, Path::new("."), auth.as_deref(), &arguments).await?;
    }
    let commit = run_git(cancel, &root, None, &["rev-parse", "HEAD"]).await?;
    let listing = list(&root, &root, 1).await?;
    Ok(CloneResult {
        id: parsed.id(),
        commit,
        listing,
        root,
    })
}

fn git_auth(forges: &BTreeMap<String, Forge>, host: &str) -> Option<String> {
    forges.values().find_map(|forge| {
        let forge_host = Url::parse(&forge.base_url)
            .ok()
            .and_then(|url| url.host_str().map(str::to_owned));
        let matches = forge_host.as_deref() == Some(host)
            || matches!(forge.kind, ForgeKind::GitHub)
                && forge_host
                    .as_deref()
                    .and_then(|value| value.strip_prefix("api."))
                    == Some(host);
        matches.then(|| {
            forge.token.as_ref().map(|token| match forge.kind {
                ForgeKind::GitHub => format!("Authorization: Bearer {token}"),
                ForgeKind::Forgejo => format!("Authorization: token {token}"),
            })
        })?
    })
}

async fn run_git(
    cancel: &CancellationToken,
    cwd: &Path,
    auth: Option<&str>,
    args: &[&str],
) -> Result<String> {
    let mut command = Command::new("git");
    if let Some(header) = auth {
        command.args(["-c", &format!("http.extraHeader={header}")]);
    }
    command
        .args(args)
        .current_dir(cwd)
        .env("GIT_TERMINAL_PROMPT", "0")
        .kill_on_drop(true);
    let output = tokio::select! {
        () = cancel.cancelled() => bail!("git command cancelled"),
        result = tokio::time::timeout(GIT_TIMEOUT, command.output()) => {
            result.context("git command timed out")?.context("failed to run git")?
        }
    };
    if !output.status.success() {
        bail!(
            "git failed: {}",
            String::from_utf8_lossy(&output.stderr).trim()
        )
    }
    Ok(String::from_utf8_lossy(&output.stdout).into_owned())
}

fn path_text(path: &Path) -> Result<&str> {
    path.to_str()
        .ok_or_else(|| anyhow!("path is not valid UTF-8"))
}
