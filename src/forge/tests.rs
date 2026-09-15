use super::*;
use wiremock::matchers::{body_json, header, method, path, query_param, query_param_is_missing};
use wiremock::{Match, Mock, MockServer, Request, ResponseTemplate};

struct SingleAccept(&'static str);

impl Match for SingleAccept {
    fn matches(&self, request: &Request) -> bool {
        let mut values = request.headers.get_all("accept").iter();
        values.next().and_then(|value| value.to_str().ok()) == Some(self.0)
            && values.next().is_none()
    }
}

fn config(kind: ForgeKind, url: String) -> ForgeConfig {
    ForgeConfig {
        kind,
        url,
        token: "secret".to_owned(),
        default_repo: None,
    }
}

fn issue() -> serde_json::Value {
    serde_json::json!({
        "number": 12,
        "title": "A problem",
        "body": null,
        "state": "open",
        "html_url": "https://forge/owner/repo/issues/12",
        "user": { "login": "alice" },
        "labels": [{ "name": "bug" }]
    })
}

#[tokio::test]
async fn github_search_uses_search_api_and_headers() -> Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/search/issues"))
        .and(query_param(
            "q",
            "needle repo:owner/repo state:open label:bug",
        ))
        .and(query_param("per_page", "5"))
        .and(header("authorization", "Bearer secret"))
        .and(header("accept", "application/vnd.github+json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!({
            "items": [issue()]
        })))
        .mount(&server)
        .await;
    let forge = Forge::new(&config(ForgeKind::GitHub, server.uri()))?;
    let results = forge
        .search_issues(
            "owner/repo",
            "needle",
            IssueState::Open,
            &["bug".to_owned()],
            5,
        )
        .await?;
    assert_eq!(results.len(), 1);
    assert_eq!(results[0].body, "");
    Ok(())
}

#[tokio::test]
async fn forgejo_search_uses_repository_api() -> Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/owner/repo/issues"))
        .and(query_param_is_missing("q"))
        .and(query_param_is_missing("labels"))
        .and(query_param("state", "all"))
        .and(header("authorization", "token secret"))
        .respond_with(ResponseTemplate::new(200).set_body_json(vec![issue()]))
        .mount(&server)
        .await;
    let forge = Forge::new(&config(ForgeKind::Forgejo, server.uri()))?;
    let results = forge
        .search_issues("owner/repo", "", IssueState::All, &[], 10)
        .await?;
    assert_eq!(results[0].number, 12);
    Ok(())
}

#[tokio::test]
async fn github_pull_diff_sends_one_diff_accept_header() -> Result<()> {
    let server = MockServer::start().await;
    let pull = serde_json::json!({
        "number": 3,
        "title": "Change",
        "body": "Description",
        "state": "open",
        "html_url": "https://forge/owner/repo/pull/3",
        "user": { "login": "alice" }
    });
    Mock::given(method("GET"))
        .and(path("/repos/owner/repo/pulls/3"))
        .and(SingleAccept("application/vnd.github+json"))
        .respond_with(ResponseTemplate::new(200).set_body_json(pull))
        .expect(1)
        .mount(&server)
        .await;
    Mock::given(method("GET"))
        .and(path("/repos/owner/repo/pulls/3"))
        .and(SingleAccept("application/vnd.github.v3.diff"))
        .respond_with(ResponseTemplate::new(200).set_body_string("diff --git a/a b/a\n"))
        .expect(1)
        .mount(&server)
        .await;
    let forge = Forge::new(&config(ForgeKind::GitHub, server.uri()))?;
    let pull = forge.get_pull_request("owner/repo", 3).await?;
    assert!(pull.diff.starts_with("diff --git"));
    Ok(())
}

#[tokio::test]
async fn forgejo_create_resolves_label_names_to_ids() -> Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/api/v1/repos/owner/repo/labels"))
        .respond_with(ResponseTemplate::new(200).set_body_json(serde_json::json!([
            { "id": 7, "name": "bug" },
            { "id": 9, "name": "help wanted" }
        ])))
        .mount(&server)
        .await;
    Mock::given(method("POST"))
        .and(path("/api/v1/repos/owner/repo/issues"))
        .and(body_json(serde_json::json!({
            "title": "Title",
            "body": "Body",
            "labels": [7]
        })))
        .respond_with(ResponseTemplate::new(201).set_body_json(issue()))
        .mount(&server)
        .await;
    let forge = Forge::new(&config(ForgeKind::Forgejo, server.uri()))?;
    let created = forge
        .create_issue("owner/repo", "Title", "Body", &["bug".to_owned()])
        .await?;
    assert_eq!(created.number, 12);
    Ok(())
}

#[tokio::test]
async fn extracts_provider_error_message() -> Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("GET"))
        .and(path("/repos/owner/repo/issues/12"))
        .respond_with(
            ResponseTemplate::new(403)
                .set_body_json(serde_json::json!({ "message": "rate limited" })),
        )
        .mount(&server)
        .await;
    let forge = Forge::new(&config(ForgeKind::GitHub, server.uri()))?;
    let error = forge
        .get_issue("owner/repo", 12)
        .await
        .expect_err("request should fail");
    assert!(error.to_string().contains("403 Forbidden: rate limited"));
    Ok(())
}
