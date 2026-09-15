use super::*;

fn forge(kind: ForgeKind, url: &str) -> Result<Forge> {
    Forge::new(
        "test",
        &ForgeConfig {
            kind,
            url: url.to_owned(),
            token: "secret".to_owned(),
            default_repo: None,
        },
    )
}

#[test]
fn api_authorization_is_scoped_to_the_forge_origin() -> Result<()> {
    let github = forge(ForgeKind::GitHub, "https://api.github.com")?;
    let forgejo = forge(ForgeKind::Forgejo, "https://code.example.com")?;
    let url = |value: &str| Url::parse(value).expect("test URLs are valid");

    assert_eq!(
        github
            .api_authorization(&url("https://api.github.com/repos/o/n/issues"))
            .as_deref(),
        Some("Bearer secret")
    );
    assert_eq!(
        forgejo
            .api_authorization(&url("https://code.example.com/api/v1/repos/o/n"))
            .as_deref(),
        Some("token secret")
    );
    assert!(
        github
            .api_authorization(&url("https://api.github.com.evil.com/"))
            .is_none()
    );
    assert!(
        forgejo
            .api_authorization(&url("http://code.example.com/api/v1"))
            .is_none()
    );
    assert_eq!(forgejo.api_base(), "https://code.example.com/api/v1");
    Ok(())
}

#[test]
fn git_authorization_uses_provider_specific_basic_credentials() -> Result<()> {
    let github = forge(ForgeKind::GitHub, "https://api.github.com")?;
    let forgejo = forge(ForgeKind::Forgejo, "https://code.example.com")?;
    assert_eq!(
        github.git_authorization("github.com").as_deref(),
        Some("Basic eC1hY2Nlc3MtdG9rZW46c2VjcmV0")
    );
    assert_eq!(
        forgejo.git_authorization("code.example.com").as_deref(),
        Some("Basic b2F1dGgyOnNlY3JldA==")
    );
    assert!(github.git_authorization("gitlab.com").is_none());
    Ok(())
}
