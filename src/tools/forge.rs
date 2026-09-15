use std::sync::Arc;

use anyhow::{Context, Result, bail};
use schemars::JsonSchema;
use serde::Deserialize;

use super::{Tool, ToolOutput};
use crate::chat::Run;
use crate::forge::{Forge, IssueState, validate_repo};
use crate::text::truncate_chars;

const DEFAULT_LIMIT: u8 = 20;
const MAX_LIMIT: u8 = 100;
const PREVIEW_BODY_LIMIT: usize = 1_500;

pub fn tools(run: &Arc<Run>) -> Vec<Tool> {
    let configured = run
        .app
        .forges
        .iter()
        .map(|(name, forge)| {
            forge
                .default_repo
                .as_ref()
                .map_or_else(|| name.clone(), |repo| format!("{name} (default {repo})"))
        })
        .collect::<Vec<_>>()
        .join(", ");
    let suffix = format!(" Configured forges: {configured}.");
    vec![
        Tool::new(
            "forge_search_issues",
            format!(
                "Search issues and pull requests when you need to find existing forge work.{suffix}"
            ),
            run.clone(),
            search_issues,
        ),
        Tool::new(
            "forge_get_issue",
            format!(
                "Get an issue and up to 50 comments when you need its full discussion.{suffix}"
            ),
            run.clone(),
            get_issue,
        ),
        Tool::new(
            "forge_list_pull_requests",
            format!("List pull requests when you need to inspect current or past changes.{suffix}"),
            run.clone(),
            list_pull_requests,
        ),
        Tool::new(
            "forge_get_pull_request",
            format!(
                "Get a pull request and its unified diff when you need to review its details or code changes.{suffix}"
            ),
            run.clone(),
            get_pull_request,
        ),
        Tool::new(
            "forge_create_issue",
            format!(
                "Create an issue when the user wants work tracked. This requires approval.{suffix}"
            ),
            run.clone(),
            create_issue,
        ),
        Tool::new(
            "forge_comment",
            format!(
                "Comment on an issue or pull request when the user wants to respond on the forge. This requires approval.{suffix}"
            ),
            run.clone(),
            comment,
        ),
    ]
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct SearchIssues {
    forge: String,
    repo: Option<String>,
    query: String,
    state: IssueState,
    #[serde(default)]
    labels: Vec<String>,
    limit: Option<u8>,
}

async fn search_issues(run: Arc<Run>, args: SearchIssues) -> Result<ToolOutput> {
    let selected = selected(&run, &args.forge, args.repo.as_deref())?;
    let limit = limit(args.limit)?;
    let issues = selected
        .forge
        .search_issues(selected.repo, &args.query, args.state, &args.labels, limit)
        .await?;
    json_output(&issues)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct GetNumbered {
    forge: String,
    repo: Option<String>,
    number: u64,
}

async fn get_issue(run: Arc<Run>, args: GetNumbered) -> Result<ToolOutput> {
    let selected = selected(&run, &args.forge, args.repo.as_deref())?;
    json_output(&selected.forge.get_issue(selected.repo, args.number).await?)
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct ListPullRequests {
    forge: String,
    repo: Option<String>,
    state: IssueState,
    limit: Option<u8>,
}

async fn list_pull_requests(run: Arc<Run>, args: ListPullRequests) -> Result<ToolOutput> {
    let selected = selected(&run, &args.forge, args.repo.as_deref())?;
    json_output(
        &selected
            .forge
            .list_pull_requests(selected.repo, args.state, limit(args.limit)?)
            .await?,
    )
}

async fn get_pull_request(run: Arc<Run>, args: GetNumbered) -> Result<ToolOutput> {
    let selected = selected(&run, &args.forge, args.repo.as_deref())?;
    json_output(
        &selected
            .forge
            .get_pull_request(selected.repo, args.number)
            .await?,
    )
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct CreateIssue {
    forge: String,
    repo: Option<String>,
    title: String,
    body: String,
    #[serde(default)]
    labels: Vec<String>,
}

async fn create_issue(run: Arc<Run>, args: CreateIssue) -> Result<ToolOutput> {
    let selected = selected(&run, &args.forge, args.repo.as_deref())?;
    let preview = format!(
        "Create issue\nForge: {}\nRepo: {}\nTitle: {}\nLabels: {}\n\n{}",
        args.forge,
        selected.repo,
        args.title,
        display_labels(&args.labels),
        truncate_chars(&args.body, PREVIEW_BODY_LIMIT)
    );
    run.approve("forge_create_issue", &preview).await?;
    let body = with_footer(&run, &args.body);
    let issue = selected
        .forge
        .create_issue(selected.repo, &args.title, &body, &args.labels)
        .await?;
    Ok(ToolOutput::text(format!(
        "Created issue #{}: {}",
        issue.number, issue.html_url
    )))
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Comment {
    forge: String,
    repo: Option<String>,
    number: u64,
    body: String,
}

async fn comment(run: Arc<Run>, args: Comment) -> Result<ToolOutput> {
    let selected = selected(&run, &args.forge, args.repo.as_deref())?;
    let preview = format!(
        "Comment on {} {} #{}\n\n{}",
        args.forge,
        selected.repo,
        args.number,
        truncate_chars(&args.body, PREVIEW_BODY_LIMIT)
    );
    run.approve("forge_comment", &preview).await?;
    let url = selected
        .forge
        .comment(selected.repo, args.number, &with_footer(&run, &args.body))
        .await?;
    Ok(ToolOutput::text(format!("Created comment: {url}")))
}

struct ForgeSelection<'a> {
    forge: &'a Forge,
    repo: &'a str,
}

fn selected<'a>(run: &'a Run, name: &str, repo: Option<&'a str>) -> Result<ForgeSelection<'a>> {
    let forge = run
        .app
        .forges
        .get(name)
        .with_context(|| format!("forge `{name}` is not configured"))?;
    let repo = repo
        .or(forge.default_repo.as_deref())
        .context("repo is required because this forge has no default_repo")?;
    validate_repo(repo)?;
    Ok(ForgeSelection { forge, repo })
}

fn limit(value: Option<u8>) -> Result<u8> {
    let limit = value.unwrap_or(DEFAULT_LIMIT);
    if !(1..=MAX_LIMIT).contains(&limit) {
        bail!("limit must be between 1 and {MAX_LIMIT}")
    }
    Ok(limit)
}

fn json_output(value: &impl serde::Serialize) -> Result<ToolOutput> {
    Ok(ToolOutput::text(
        serde_json::to_string_pretty(value).context("failed to format forge result")?,
    ))
}

fn display_labels(labels: &[String]) -> String {
    if labels.is_empty() {
        "none".to_owned()
    } else {
        labels.join(", ")
    }
}

fn with_footer(run: &Run, body: &str) -> String {
    format!(
        "{body}\n\nCreated from Discord by {} via Reseam Bot: https://discord.com/channels/{}/{}/{}",
        run.invoker.display_name(),
        run.guild_id,
        run.channel_id,
        run.reply_to
    )
}
