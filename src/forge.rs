use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{Method, RequestBuilder, StatusCode};
use schemars::JsonSchema;
use serde::{Deserialize, Deserializer, Serialize, de::DeserializeOwned};

use crate::config::{ForgeConfig, ForgeKind};
use crate::text::{truncate_chars, truncate_output};

const BODY_LIMIT: usize = 4_000;
const COMMENT_LIMIT: usize = 50;

#[derive(Clone)]
pub struct Forge {
    pub kind: ForgeKind,
    pub base_url: String,
    pub token: Option<String>,
    pub default_repo: Option<String>,
    client: reqwest::Client,
}

#[derive(Clone, Copy, Deserialize, JsonSchema)]
#[serde(rename_all = "lowercase")]
pub enum IssueState {
    Open,
    Closed,
    All,
}

impl IssueState {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Open => "open",
            Self::Closed => "closed",
            Self::All => "all",
        }
    }
}

#[derive(Debug, Deserialize, Serialize)]
pub struct User {
    pub login: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Label {
    pub name: String,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Issue {
    pub number: u64,
    pub title: String,
    #[serde(default, deserialize_with = "string_or_default")]
    pub body: String,
    pub state: String,
    pub html_url: String,
    pub user: User,
    #[serde(default)]
    pub labels: Vec<Label>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Comment {
    pub user: User,
    #[serde(default, deserialize_with = "string_or_default")]
    pub body: String,
    pub html_url: String,
}

#[derive(Debug, Serialize)]
pub struct IssueWithComments {
    pub issue: Issue,
    pub comments: Vec<Comment>,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct PullRequest {
    pub number: u64,
    pub title: String,
    #[serde(default, deserialize_with = "string_or_default")]
    pub body: String,
    pub state: String,
    pub html_url: String,
    pub user: User,
}

#[derive(Debug, Serialize)]
pub struct PullRequestWithDiff {
    pub pull_request: PullRequest,
    pub diff: String,
}

#[derive(Deserialize)]
struct SearchResponse {
    items: Vec<Issue>,
}

#[derive(Deserialize)]
struct ProviderLabel {
    id: u64,
    name: String,
}

#[derive(Deserialize)]
struct CreatedComment {
    html_url: String,
}

#[derive(Serialize)]
struct CreateComment<'a> {
    body: &'a str,
}

#[derive(Deserialize)]
struct ErrorResponse {
    message: String,
}

#[derive(Serialize)]
struct CreateIssue<'a, T> {
    title: &'a str,
    body: &'a str,
    labels: T,
}

impl Forge {
    pub fn new(config: &ForgeConfig) -> Result<Self> {
        reqwest::Url::parse(&config.url).context("forge URL is invalid")?;
        if let Some(repo) = &config.default_repo {
            validate_repo(repo).context("forge default_repo is invalid")?;
        }
        let client = reqwest::Client::builder()
            .connect_timeout(Duration::from_secs(30))
            .timeout(Duration::from_secs(60))
            .user_agent("Reseam-Bot/0.1")
            .build()
            .context("failed to build forge HTTP client")?;
        Ok(Self {
            kind: config.kind,
            base_url: config.url.trim_end_matches('/').to_owned(),
            token: (!config.token.is_empty()).then(|| config.token.clone()),
            default_repo: config.default_repo.clone(),
            client,
        })
    }

    pub async fn search_issues(
        &self,
        repo: &str,
        query: &str,
        state: IssueState,
        labels: &[String],
        limit: u8,
    ) -> Result<Vec<Issue>> {
        match self.kind {
            ForgeKind::GitHub => {
                let mut terms = vec![query.to_owned(), format!("repo:{repo}")];
                if !matches!(state, IssueState::All) {
                    terms.push(format!("state:{}", state.as_str()));
                }
                terms.extend(labels.iter().map(|label| format!("label:{label}")));
                let response: SearchResponse = self
                    .json(
                        self.request(Method::GET, "search/issues")
                            .query(&[("q", terms.join(" ")), ("per_page", limit.to_string())]),
                    )
                    .await?;
                Ok(response.items)
            }
            ForgeKind::Forgejo => {
                let mut request = self
                    .request(Method::GET, &format!("repos/{repo}/issues"))
                    .query(&[
                        ("state", state.as_str().to_owned()),
                        ("limit", limit.to_string()),
                    ]);
                if !query.is_empty() {
                    request = request.query(&[("q", query)]);
                }
                if !labels.is_empty() {
                    request = request.query(&[("labels", labels.join(","))]);
                }
                self.json(request).await
            }
        }
    }

    pub async fn get_issue(&self, repo: &str, number: u64) -> Result<IssueWithComments> {
        let mut issue: Issue = self
            .json(self.request(Method::GET, &format!("repos/{repo}/issues/{number}")))
            .await?;
        issue.body = truncate_chars(&issue.body, BODY_LIMIT);
        let mut comments: Vec<Comment> = self
            .json(
                self.request(
                    Method::GET,
                    &format!("repos/{repo}/issues/{number}/comments"),
                )
                .query(&[("limit", COMMENT_LIMIT), ("per_page", COMMENT_LIMIT)]),
            )
            .await?;
        comments.truncate(COMMENT_LIMIT);
        comments
            .iter_mut()
            .for_each(|comment| comment.body = truncate_chars(&comment.body, BODY_LIMIT));
        Ok(IssueWithComments { issue, comments })
    }

    pub async fn create_issue(
        &self,
        repo: &str,
        title: &str,
        body: &str,
        labels: &[String],
    ) -> Result<Issue> {
        let request = self.request(Method::POST, &format!("repos/{repo}/issues"));
        match self.kind {
            ForgeKind::GitHub => {
                self.json(request.json(&CreateIssue {
                    title,
                    body,
                    labels,
                }))
                .await
            }
            ForgeKind::Forgejo => {
                let available: Vec<ProviderLabel> = self
                    .json(
                        self.request(Method::GET, &format!("repos/{repo}/labels"))
                            .query(&[("limit", 100), ("per_page", 100)]),
                    )
                    .await?;
                let ids = labels
                    .iter()
                    .map(|wanted| {
                        available
                            .iter()
                            .find(|label| label.name == *wanted)
                            .map(|label| label.id)
                            .ok_or_else(|| anyhow!("label `{wanted}` does not exist in {repo}"))
                    })
                    .collect::<Result<Vec<_>>>()?;
                self.json(request.json(&CreateIssue {
                    title,
                    body,
                    labels: ids,
                }))
                .await
            }
        }
    }

    pub async fn comment(&self, repo: &str, number: u64, body: &str) -> Result<String> {
        let comment: CreatedComment = self
            .json(
                self.request(
                    Method::POST,
                    &format!("repos/{repo}/issues/{number}/comments"),
                )
                .json(&CreateComment { body }),
            )
            .await?;
        Ok(comment.html_url)
    }

    pub async fn list_pull_requests(
        &self,
        repo: &str,
        state: IssueState,
        limit: u8,
    ) -> Result<Vec<PullRequest>> {
        self.json(
            self.request(Method::GET, &format!("repos/{repo}/pulls"))
                .query(&[
                    ("state", state.as_str().to_owned()),
                    ("limit", limit.to_string()),
                    ("per_page", limit.to_string()),
                ]),
        )
        .await
    }

    pub async fn get_pull_request(&self, repo: &str, number: u64) -> Result<PullRequestWithDiff> {
        let pull_request = self
            .json(self.request(Method::GET, &format!("repos/{repo}/pulls/{number}")))
            .await?;
        let diff_path = match self.kind {
            ForgeKind::GitHub => format!("repos/{repo}/pulls/{number}"),
            ForgeKind::Forgejo => format!("repos/{repo}/pulls/{number}.diff"),
        };
        let request = match self.kind {
            ForgeKind::GitHub => self
                .base_request(Method::GET, &diff_path)
                .header("Accept", "application/vnd.github.v3.diff"),
            ForgeKind::Forgejo => self.request(Method::GET, &diff_path),
        };
        let diff = truncate_output(&self.text(request).await?);
        Ok(PullRequestWithDiff { pull_request, diff })
    }

    fn request(&self, method: Method, path: &str) -> RequestBuilder {
        let request = self.base_request(method, path);
        if matches!(self.kind, ForgeKind::GitHub) {
            request.header("Accept", "application/vnd.github+json")
        } else {
            request
        }
    }

    fn base_request(&self, method: Method, path: &str) -> RequestBuilder {
        let url = match self.kind {
            ForgeKind::GitHub => format!("{}/{path}", self.base_url),
            ForgeKind::Forgejo => format!("{}/api/v1/{path}", self.base_url),
        };
        let mut request = self.client.request(method, url);
        if let Some(token) = &self.token {
            let authorization = match self.kind {
                ForgeKind::GitHub => format!("Bearer {token}"),
                ForgeKind::Forgejo => format!("token {token}"),
            };
            request = request.header("Authorization", authorization);
        }
        if matches!(self.kind, ForgeKind::GitHub) {
            request = request.header("X-GitHub-Api-Version", "2022-11-28");
        }
        request
    }

    async fn json<T: DeserializeOwned>(&self, request: RequestBuilder) -> Result<T> {
        let response = request.send().await.context("forge request failed")?;
        let status = response.status();
        if !status.is_success() {
            return Err(response_error(status, response).await);
        }
        response.json().await.context("invalid forge response")
    }

    async fn text(&self, request: RequestBuilder) -> Result<String> {
        let response = request.send().await.context("forge request failed")?;
        let status = response.status();
        if !status.is_success() {
            return Err(response_error(status, response).await);
        }
        response.text().await.context("invalid forge text response")
    }
}

async fn response_error(status: StatusCode, response: reqwest::Response) -> anyhow::Error {
    match response.json::<ErrorResponse>().await {
        Ok(error) => anyhow!("forge returned {status}: {}", error.message),
        Err(_) => anyhow!("forge returned {status}"),
    }
}

fn string_or_default<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> std::result::Result<String, D::Error> {
    Ok(Option::<String>::deserialize(deserializer)?.unwrap_or_default())
}

pub fn validate_repo(repo: &str) -> Result<()> {
    let mut parts = repo.split('/');
    if parts.next().is_some_and(|part| !part.is_empty())
        && parts.next().is_some_and(|part| !part.is_empty())
        && parts.next().is_none()
    {
        return Ok(());
    }
    bail!("repo must be owner/name")
}

#[cfg(test)]
mod tests;
