use std::path::Path;
use std::time::{Duration, Instant};

use anyhow::Result;
use tokio_util::sync::CancellationToken;

use super::execute;

#[tokio::test]
async fn timeout_kills_the_process_group_promptly() -> Result<()> {
    let start = Instant::now();
    let output = execute(
        "sleep 30 & sleep 30; wait",
        Path::new("/tmp"),
        1,
        &CancellationToken::new(),
    )
    .await?;
    assert!(output.starts_with("timed out after 1s"));
    assert!(start.elapsed() < Duration::from_secs(5));
    Ok(())
}

#[tokio::test]
async fn timeout_does_not_wait_forever_for_an_escaped_pipe() -> Result<()> {
    let start = Instant::now();
    let output = execute(
        "setsid sleep 7 & sleep 30",
        Path::new("/tmp"),
        1,
        &CancellationToken::new(),
    )
    .await?;
    assert!(output.starts_with("timed out after 1s"));
    assert!(start.elapsed() < Duration::from_secs(8));
    Ok(())
}

#[tokio::test]
async fn clears_environment_and_captures_ordered_output() -> Result<()> {
    let output = execute(
        "printf 'one\\n'; printf 'two\\n' >&2; env | sort",
        Path::new("/tmp"),
        5,
        &CancellationToken::new(),
    )
    .await?;
    assert!(output.starts_with("exit status: 0\none\ntwo\n"));
    let variables = output.lines().skip(3).collect::<Vec<_>>();
    assert!(variables.iter().all(|line| {
        ["HOME=", "LANG=", "PATH=", "PWD=", "SHLVL=", "TERM=", "_="]
            .iter()
            .any(|prefix| line.starts_with(prefix))
    }));
    Ok(())
}
