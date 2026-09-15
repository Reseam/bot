use std::ffi::OsString;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use rustix::process::{Pid, Signal, kill_process_group};
use schemars::JsonSchema;
use serde::Deserialize;
use tokio::io::AsyncReadExt;
use tokio::process::Command;
use tokio_util::sync::CancellationToken;

use super::repo::{canonical_within, validate_relative};
use super::{Tool, ToolOutput};
use crate::chat::Run;
use crate::text::truncate_output;

const MAX_TIMEOUT_SECS: u64 = 600;
const OUTPUT_DRAIN_TIMEOUT: Duration = Duration::from_secs(5);

pub fn tools(run: &Arc<Run>) -> Vec<Tool> {
    vec![Tool::new::<Shell, _, _, _>(
        "shell",
        "Run a bash command in the isolated workspace or a cloned repository after approval.",
        run.clone(),
        shell,
    )]
}

#[derive(Deserialize, JsonSchema)]
#[serde(deny_unknown_fields)]
struct Shell {
    command: String,
    timeout_secs: Option<u64>,
    workdir: Option<String>,
}

async fn shell(run: Arc<Run>, args: Shell) -> Result<ToolOutput> {
    let timeout = args
        .timeout_secs
        .unwrap_or(run.app.config.shell.timeout_secs);
    if !(1..=MAX_TIMEOUT_SECS).contains(&timeout) {
        bail!("timeout_secs must be between 1 and {MAX_TIMEOUT_SECS}")
    }
    let workdir = resolve_workdir(&run, args.workdir.as_deref()).await?;
    let preview = format!(
        "Run in {}:\n```sh\n{}\n```",
        workdir.display(),
        args.command
    );
    run.approve("shell", &preview).await?;
    Ok(ToolOutput::text(
        execute(&args.command, &workdir, timeout, &run.cancel).await?,
    ))
}

async fn resolve_workdir(run: &Run, requested: Option<&str>) -> Result<PathBuf> {
    let workspace = run.app.config.data_dir.join("workspace");
    tokio::fs::create_dir_all(&workspace)
        .await
        .context("failed to create shell workspace")?;
    let Some(requested) = requested else {
        return workspace
            .canonicalize()
            .context("failed to resolve workspace");
    };
    validate_relative(requested)?;
    let repo_candidate = run.app.config.data_dir.join("repos").join(requested);
    if repo_candidate.is_dir() {
        return canonical_within(&run.app.config.data_dir.join("repos"), &repo_candidate);
    }
    let candidate = workspace.join(requested);
    tokio::fs::create_dir_all(&candidate)
        .await
        .context("failed to create shell work directory")?;
    canonical_within(&workspace, &candidate)
}

async fn execute(
    command: &str,
    workdir: &Path,
    timeout_secs: u64,
    cancel: &CancellationToken,
) -> Result<String> {
    let mut process = Command::new("bash");
    process
        .args(["-c", &format!("exec 2>&1\n{command}")])
        .current_dir(workdir)
        .env_clear()
        .envs(preserved_environment())
        .env("TERM", "dumb")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .process_group(0);
    let mut child = process.spawn().context("failed to start bash")?;
    let raw_pid = child.id().context("bash has no process ID")?;
    let raw_pid = i32::try_from(raw_pid).context("bash process ID exceeds i32")?;
    let pid = Pid::from_raw(raw_pid).context("bash has an invalid process ID")?;
    let mut stdout = child.stdout.take().context("bash stdout is unavailable")?;
    let mut bytes = Vec::new();
    let mut chunk = [0; 8 * 1024];
    let mut stdout_open = true;
    let wait = child.wait();
    let deadline = tokio::time::sleep(Duration::from_secs(timeout_secs));
    tokio::pin!(wait, deadline);
    enum Outcome {
        Finished(std::process::ExitStatus),
        TimedOut,
        Killed,
    }
    let outcome = loop {
        tokio::select! {
            result = &mut wait => {
                break Outcome::Finished(result.context("failed to wait for bash")?);
            }
            result = stdout.read(&mut chunk), if stdout_open => {
                let read = result.context("failed to read bash output")?;
                if read == 0 {
                    stdout_open = false;
                } else {
                    bytes.extend_from_slice(&chunk[..read]);
                }
            }
            () = &mut deadline => break Outcome::TimedOut,
            () = cancel.cancelled() => break Outcome::Killed,
        }
    };
    if matches!(outcome, Outcome::TimedOut | Outcome::Killed) {
        kill_group(pid)?;
        let reap = tokio::time::timeout(OUTPUT_DRAIN_TIMEOUT, &mut wait);
        let drain = drain_output(&mut stdout, &mut bytes, stdout_open);
        let (reap, drain) = tokio::join!(reap, drain);
        if let Ok(result) = reap {
            result.context("failed to wait for killed bash")?;
        }
        drain?;
    } else {
        drain_output(&mut stdout, &mut bytes, stdout_open).await?;
    }
    let status = match outcome {
        Outcome::Finished(exit_status) => exit_status.code().map_or_else(
            || "killed".to_owned(),
            |code| format!("exit status: {code}"),
        ),
        Outcome::TimedOut => format!("timed out after {timeout_secs}s"),
        Outcome::Killed => "killed".to_owned(),
    };
    let output = String::from_utf8_lossy(&bytes);
    Ok(truncate_output(&format!("{status}\n{output}")))
}

async fn drain_output(
    stdout: &mut tokio::process::ChildStdout,
    bytes: &mut Vec<u8>,
    stdout_open: bool,
) -> Result<()> {
    if stdout_open
        && let Ok(result) =
            tokio::time::timeout(OUTPUT_DRAIN_TIMEOUT, stdout.read_to_end(bytes)).await
    {
        result.context("failed to read remaining bash output")?;
    }
    Ok(())
}

fn kill_group(pid: Pid) -> Result<()> {
    kill_process_group(pid, Signal::KILL).context("failed to kill shell process group")
}

fn preserved_environment() -> Vec<(&'static str, OsString)> {
    ["PATH", "HOME", "LANG"]
        .into_iter()
        .filter_map(|name| std::env::var_os(name).map(|value| (name, value)))
        .collect()
}

#[cfg(test)]
mod tests {
    use std::time::Instant;

    use super::*;

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
}
