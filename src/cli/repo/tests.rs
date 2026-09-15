use super::Repository;

#[test]
fn repository_urls_cannot_escape_the_repos_directory() {
    let parsed = Repository::parse("https://github.com/serenity-rs/poise.git").expect("valid URL");
    assert_eq!(
        (
            parsed.host.as_str(),
            parsed.owner.as_str(),
            parsed.name.as_str()
        ),
        ("github.com", "serenity-rs", "poise")
    );
    for invalid in [
        "ssh://github.com/owner/repo",
        "https://github.com/owner/../repo",
        "https://github.com/owner/%2e%2e",
        "https://github.com/.git/repo",
        "https://github.com/owner/repo/extra",
        "https://token@github.com/owner/repo",
    ] {
        assert!(Repository::parse(invalid).is_err(), "{invalid}");
    }
}
