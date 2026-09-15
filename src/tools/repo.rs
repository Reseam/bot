use std::path::{Component, Path, PathBuf};
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use ignore::WalkBuilder;
use ignore::overrides::OverrideBuilder;
use regex::RegexBuilder;
use schemars::JsonSchema;
use serde::Deserialize;

use super::{Tool, ToolOutput};
use crate::chat::Run;
use crate::text::truncate_chars;

const MAX_LIST_ENTRIES: usize = 500;
const MAX_FILE_BYTES: u64 = 2 * 1024 * 1024;

mod clone;

pub use clone::RepoLocks;

pub fn tools(run: &Arc<Run>) -> Vec<Tool> {
    vec![
        clone::tool(run),
        Tool::new::<ListRepo, _, _, _>(
            "repo_list",
            "List files and directories in a cloned repository, respecting .gitignore.",
            run.clone(),
            list_repo,
        ),
        Tool::new::<ReadRepo, _, _, _>(
            "repo_read",
            "Read numbered lines from a non-binary file in a cloned repository.",
            run.clone(),
            read_repo,
        ),
        Tool::new::<GrepRepo, _, _, _>(
            "repo_grep",
            "Search cloned repository files with a regular expression.",
            run.clone(),
            grep_repo,
        ),
    ]
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListRepo {
    repo: String,
    path: Option<String>,
    depth: Option<usize>,
}

async fn list_repo(run: Arc<Run>, args: ListRepo) -> Result<ToolOutput> {
    let root = repo_root(&run, &args.repo).await?;
    let path = safe_path(&root, args.path.as_deref().unwrap_or(""))?;
    let depth = args.depth.unwrap_or(2);
    if !(1..=6).contains(&depth) {
        bail!("depth must be between 1 and 6")
    }
    Ok(ToolOutput::text(list(&root, &path, depth).await?))
}

async fn list(root: &Path, path: &Path, depth: usize) -> Result<String> {
    let root = root.to_owned();
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || {
        let mut entries = WalkBuilder::new(path)
            .standard_filters(true)
            .hidden(false)
            .filter_entry(|entry| entry.file_name() != ".git")
            .max_depth(Some(depth))
            .build()
            .filter_map(Result::ok)
            .filter(|entry| entry.depth() > 0)
            .take(MAX_LIST_ENTRIES + 1)
            .map(|entry| {
                let relative = entry.path().strip_prefix(&root).unwrap_or(entry.path());
                format!(
                    "{}{}",
                    relative.display(),
                    if entry.file_type().is_some_and(|kind| kind.is_dir()) {
                        "/"
                    } else {
                        ""
                    }
                )
            })
            .collect::<Vec<_>>();
        let truncated = entries.len() > MAX_LIST_ENTRIES;
        entries.truncate(MAX_LIST_ENTRIES);
        if truncated {
            entries.push("[truncated after 500 entries]".to_owned());
        }
        Ok(entries.join("\n"))
    })
    .await
    .context("repository listing task failed")?
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ReadRepo {
    repo: String,
    path: String,
    offset: Option<usize>,
    limit: Option<usize>,
}

async fn read_repo(run: Arc<Run>, args: ReadRepo) -> Result<ToolOutput> {
    let root = repo_root(&run, &args.repo).await?;
    let path = safe_path(&root, &args.path)?;
    let offset = args.offset.unwrap_or(1);
    let limit = args.limit.unwrap_or(400);
    if offset == 0 || !(1..=2_000).contains(&limit) {
        bail!("offset must be at least 1 and limit between 1 and 2000")
    }
    Ok(ToolOutput::text(read(&path, offset, limit).await?))
}

async fn read(path: &Path, offset: usize, limit: usize) -> Result<String> {
    let path = path.to_owned();
    tokio::task::spawn_blocking(move || -> Result<String> {
        let metadata = std::fs::metadata(&path)
            .with_context(|| format!("failed to inspect {}", path.display()))?;
        if metadata.len() > MAX_FILE_BYTES {
            bail!("refusing to read file larger than 2 MiB")
        }
        let bytes =
            std::fs::read(&path).with_context(|| format!("failed to read {}", path.display()))?;
        if bytes.contains(&0) {
            bail!("refusing to read binary file")
        }
        let text = String::from_utf8(bytes).context("file is not UTF-8 text")?;
        Ok(text
            .lines()
            .enumerate()
            .skip(offset - 1)
            .take(limit)
            .map(|(index, line)| format!("{:>6}  {line}", index + 1))
            .collect::<Vec<_>>()
            .join("\n"))
    })
    .await
    .context("repository read task failed")?
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GrepRepo {
    repo: String,
    pattern: String,
    path: Option<String>,
    glob: Option<String>,
    #[serde(default)]
    case_insensitive: bool,
    max_results: Option<usize>,
}

async fn grep_repo(run: Arc<Run>, args: GrepRepo) -> Result<ToolOutput> {
    let root = repo_root(&run, &args.repo).await?;
    let path = safe_path(&root, args.path.as_deref().unwrap_or(""))?;
    let max_results = args.max_results.unwrap_or(100);
    if !(1..=500).contains(&max_results) {
        bail!("max_results must be between 1 and 500")
    }
    let output = tokio::task::spawn_blocking(move || grep(&root, &path, &args, max_results))
        .await
        .context("repository grep task failed")??;
    Ok(ToolOutput::text(output))
}

fn grep(root: &Path, path: &Path, args: &GrepRepo, limit: usize) -> Result<String> {
    let regex = RegexBuilder::new(&args.pattern)
        .case_insensitive(args.case_insensitive)
        .build()
        .context("invalid regular expression")?;
    let mut builder = WalkBuilder::new(path);
    builder
        .standard_filters(true)
        .hidden(false)
        .filter_entry(|entry| entry.file_name() != ".git");
    if let Some(glob) = &args.glob {
        let mut overrides = OverrideBuilder::new(root);
        overrides.add(glob).context("invalid glob")?;
        builder.overrides(overrides.build().context("invalid glob")?);
    }
    let mut results = Vec::new();
    for entry in builder.build().filter_map(Result::ok) {
        let Some(kind) = entry.file_type() else {
            continue;
        };
        if !kind.is_file()
            || entry
                .metadata()
                .is_ok_and(|metadata| metadata.len() > MAX_FILE_BYTES)
        {
            continue;
        }
        let Ok(bytes) = std::fs::read(entry.path()) else {
            continue;
        };
        if bytes.contains(&0) {
            continue;
        }
        let Ok(text) = std::str::from_utf8(&bytes) else {
            continue;
        };
        let relative = entry.path().strip_prefix(root).unwrap_or(entry.path());
        for (index, line) in text
            .lines()
            .enumerate()
            .filter(|(_, line)| regex.is_match(line))
        {
            results.push(format!(
                "{}:{}: {}",
                relative.display(),
                index + 1,
                truncate_chars(line, 300)
            ));
            if results.len() > limit {
                results.truncate(limit);
                results.push(format!("[truncated after {limit} results]"));
                return Ok(results.join("\n"));
            }
        }
    }
    Ok(results.join("\n"))
}

async fn repo_root(run: &Run, repo: &str) -> Result<PathBuf> {
    validate_repo_id(repo)?;
    let base = run.app.config.data_dir.join("repos");
    let root = base.join(repo);
    tokio::task::spawn_blocking(move || canonical_within(&base, &root))
        .await
        .context("repository path task failed")?
}

fn validate_repo_id(repo: &str) -> Result<()> {
    validate_relative(repo)?;
    if Path::new(repo).components().count() != 3 {
        bail!("repo must be host/owner/name")
    }
    Ok(())
}

fn safe_path(root: &Path, requested: &str) -> Result<PathBuf> {
    validate_relative(requested)?;
    canonical_within(root, &root.join(requested))
}

pub(super) fn validate_relative(value: &str) -> Result<()> {
    let path = Path::new(value);
    if path.is_absolute()
        || path.components().any(|part| {
            matches!(
                part,
                Component::ParentDir | Component::RootDir | Component::Prefix(_)
            )
        })
    {
        bail!("path must be relative and must not contain ..")
    }
    Ok(())
}

pub(super) fn canonical_within(root: &Path, candidate: &Path) -> Result<PathBuf> {
    let canonical_root = root
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", root.display()))?;
    let canonical = candidate
        .canonicalize()
        .with_context(|| format!("failed to resolve {}", candidate.display()))?;
    if !canonical.starts_with(&canonical_root) {
        bail!("path escapes the repository")
    }
    Ok(canonical)
}

#[cfg(test)]
mod tests;
