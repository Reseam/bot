use super::clone::{RepoUrl, clone_checkout};
use super::*;

#[test]
fn validates_repository_urls() {
    assert_eq!(
        RepoUrl::parse("https://github.com/serenity-rs/poise.git")
            .expect("valid URL")
            .id(),
        "github.com/serenity-rs/poise"
    );
    assert!(RepoUrl::parse("ssh://github.com/owner/repo").is_err());
    assert!(RepoUrl::parse("https://github.com/owner/../repo").is_err());
    assert!(RepoUrl::parse("https://github.com/owner/repo/extra").is_err());
}

#[cfg(unix)]
#[test]
fn rejects_parent_and_symlink_escapes() -> Result<()> {
    use std::os::unix::fs::symlink;

    let base = std::env::temp_dir().join(format!("reseam-path-test-{}", std::process::id()));
    let root = base.join("repo");
    let outside = base.join("outside");
    std::fs::create_dir_all(&root)?;
    std::fs::create_dir_all(&outside)?;
    symlink(&outside, root.join("escape"))?;
    assert!(safe_path(&root, "../outside").is_err());
    assert!(safe_path(&root, "escape").is_err());
    std::fs::remove_dir_all(&base)?;
    Ok(())
}

#[tokio::test]
async fn listing_and_grep_skip_git_metadata() -> Result<()> {
    let base = std::env::temp_dir().join(format!("reseam-ignore-git-test-{}", std::process::id()));
    std::fs::create_dir_all(base.join(".git/objects"))?;
    std::fs::write(base.join("visible.txt"), "needle")?;
    std::fs::write(base.join(".git/objects/hidden"), "needle")?;
    let listing = list(&base, &base, 4).await?;
    assert_eq!(listing, "visible.txt");
    let args = GrepRepo {
        repo: "unused".to_owned(),
        pattern: "needle".to_owned(),
        path: None,
        glob: None,
        case_insensitive: false,
        max_results: None,
    };
    assert_eq!(grep(&base, &base, &args, 10)?, "visible.txt:1: needle");
    std::fs::remove_dir_all(base)?;
    Ok(())
}

#[tokio::test]
async fn read_rejects_files_over_the_size_limit() -> Result<()> {
    let path = std::env::temp_dir().join(format!("reseam-large-read-test-{}", std::process::id()));
    let file = std::fs::File::create(&path)?;
    file.set_len(MAX_FILE_BYTES + 1)?;
    let error = read(&path, 1, 1).await.expect_err("large file should fail");
    assert!(error.to_string().contains("larger than 2 MiB"));
    std::fs::remove_file(path)?;
    Ok(())
}

#[tokio::test]
#[ignore = "clones a public repository over the network"]
async fn live_repo_clone_grep_and_read() -> Result<()> {
    let base = std::env::temp_dir().join(format!(
        "reseam-live-repo-test-{}-{}",
        std::process::id(),
        std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)?
            .as_nanos()
    ));
    let result = clone_checkout(
        &base,
        &RepoLocks::default(),
        &std::collections::BTreeMap::new(),
        &tokio_util::sync::CancellationToken::new(),
        "https://github.com/serenity-rs/poise",
        None,
    )
    .await?;
    assert_eq!(result.id, "github.com/serenity-rs/poise");
    assert_eq!(result.commit.trim().len(), 40);
    assert!(result.listing.contains("src/"));
    let grep_args = GrepRepo {
        repo: result.id,
        pattern: "pub struct Framework".to_owned(),
        path: Some("src".to_owned()),
        glob: Some("*.rs".to_owned()),
        case_insensitive: false,
        max_results: Some(10),
    };
    let matches = grep(&result.root, &result.root.join("src"), &grep_args, 10)?;
    assert!(matches.contains("Framework"));
    let cargo = read(&result.root.join("Cargo.toml"), 1, 20).await?;
    assert!(cargo.contains("name = \"poise\""));
    std::fs::remove_dir_all(&base)?;
    Ok(())
}
