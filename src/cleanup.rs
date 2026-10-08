use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result};
use tracing::{error, info};

use crate::App;
use crate::sandbox::{repos_dir, workspaces_dir};
use crate::storage::LINK_TTL;

const INTERVAL: Duration = Duration::from_secs(8 * 60 * 60);
const IDLE: Duration = Duration::from_secs(8 * 60 * 60);

pub fn spawn(app: Arc<App>) {
    tokio::spawn(async move {
        let mut interval = tokio::time::interval(INTERVAL);
        loop {
            interval.tick().await;
            if let Err(error) = sweep(&app).await {
                error!(error = %format!("{error:#}"), "failed to clean up sandbox files");
            }
        }
    });
}

async fn sweep(app: &App) -> Result<()> {
    let data_dir = app.config.data_dir.clone();
    let (workspaces, repos) = tokio::task::spawn_blocking(move || {
        Ok::<_, anyhow::Error>((
            idle_directories(&workspaces_dir(&data_dir), 1)?,
            idle_directories(&repos_dir(&data_dir), 3)?,
        ))
    })
    .await
    .context("cleanup scan task failed")??;

    let mut removed = 0;
    for workspace in workspaces {
        let Some(conversation_id) = workspace
            .file_name()
            .and_then(|name| name.to_str())
            .and_then(|name| name.parse::<i64>().ok())
        else {
            continue;
        };
        let Some(_lock) = app.runs.try_lock(conversation_id) else {
            continue;
        };
        app.sandbox.close(conversation_id);
        remove(workspace).await?;
        removed += 1;
    }
    for repo in repos {
        let Some(_lock) = app.repo_locks.try_lock(&repo) else {
            continue;
        };
        remove(repo).await?;
        removed += 1;
    }
    info!(removed, "cleaned up idle sandbox files");
    if let Some(storage) = &app.storage {
        let removed = storage.sweep(LINK_TTL).await?;
        info!(removed, "cleaned up expired shared files");
    }
    Ok(())
}

fn idle_directories(root: &Path, depth: usize) -> Result<Vec<PathBuf>> {
    if !root.exists() {
        return Ok(Vec::new());
    }
    let mut level = vec![root.to_owned()];
    for _ in 0..depth {
        let mut next = Vec::new();
        for directory in level {
            for entry in std::fs::read_dir(&directory)
                .with_context(|| format!("failed to list {}", directory.display()))?
            {
                let entry = entry.context("failed to read directory entry")?;
                if entry
                    .file_type()
                    .context("failed to read file type")?
                    .is_dir()
                {
                    next.push(entry.path());
                }
            }
        }
        level = next;
    }
    let cutoff = SystemTime::now() - IDLE;
    Ok(level
        .into_iter()
        .filter(|path| {
            std::fs::metadata(path)
                .and_then(|metadata| metadata.modified())
                .is_ok_and(|modified| modified < cutoff)
        })
        .collect())
}

async fn remove(path: PathBuf) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        std::fs::remove_dir_all(&path)
            .with_context(|| format!("failed to remove {}", path.display()))
    })
    .await
    .context("cleanup task failed")?
}
