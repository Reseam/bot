use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use parking_lot::Mutex;

use crate::App;
use crate::attachments;
use crate::chat::Run;
use crate::config::{ModalConfig, SandboxConfig};
use crate::text::truncate_output;
use crate::tools::{ImageData, ToolOutput};

pub mod files;
mod http;
mod process;
mod protocol;

use process::{Process, Response};
use protocol::{Backend, NodeImage, Saved, ToNode};

#[derive(Clone, Copy, Debug, Eq, PartialEq, poise::ChoiceParameter, sqlx::Type)]
#[sqlx(rename_all = "kebab-case")]
pub enum SandboxKind {
    #[name = "just-bash"]
    JustBash,
    #[name = "modal"]
    Modal,
}

pub fn workspaces_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("workspaces")
}

pub fn repos_dir(data_dir: &Path) -> PathBuf {
    data_dir.join("repos")
}

pub fn touch(path: &Path) -> Result<()> {
    std::fs::File::open(path)
        .and_then(|directory| directory.set_modified(SystemTime::now()))
        .with_context(|| format!("failed to mark {} as used", path.display()))
}

pub struct Sandbox {
    entry: PathBuf,
    modal: Option<ModalConfig>,
    http: http::Http,
    process: Mutex<Option<Arc<Process>>>,
    next_id: AtomicU64,
}

pub struct Execution {
    stdout: String,
    stderr: String,
    exit_code: i32,
    images: Vec<ImageData>,
}

impl Execution {
    pub fn into_output(self) -> ToolOutput {
        let mut text = format!("exit code {}\n{}", self.exit_code, self.stdout);
        if !self.stderr.is_empty() {
            if !text.ends_with('\n') {
                text.push('\n');
            }
            text.push_str("[stderr]\n");
            text.push_str(&self.stderr);
        }
        ToolOutput {
            text: truncate_output(&text),
            images: self.images,
        }
    }
}

impl Sandbox {
    pub fn new(config: &SandboxConfig) -> Result<Self> {
        Ok(Self {
            entry: config.entry.clone(),
            modal: config.modal.clone(),
            http: http::Http::new()?,
            process: Mutex::new(None),
            next_id: AtomicU64::new(1),
        })
    }

    pub async fn exec(
        &self,
        run: &Arc<Run>,
        command: &str,
        timeout: Duration,
    ) -> Result<Execution> {
        let backend = match run.sandbox {
            SandboxKind::JustBash => just_bash(run).await?,
            SandboxKind::Modal => Backend::Modal {
                image: run.sandbox_image.clone(),
            },
        };
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = ToNode::Exec {
            id,
            sandbox: run.conversation_id,
            command: command.to_owned(),
            timeout_ms: timeout.as_millis(),
            backend,
        };
        let Response::Exec(result) = self.process(&run.app)?.request(id, message).await? else {
            bail!("the sandbox service answered a command with a save result");
        };

        let mut stderr = result.stderr;
        let mut images = Vec::with_capacity(result.images.len());
        for image in result.images {
            match decode_image(run.model.config.vision, image).await {
                Ok(image) => images.push(image),
                Err(error) => stderr.push_str(&format!("{error:#}\n")),
            }
        }
        Ok(Execution {
            stdout: result.stdout,
            stderr,
            exit_code: result.exit_code,
            images,
        })
    }

    pub async fn save(&self, run: &Run) -> Result<Option<String>> {
        let Some(process) = self.running() else {
            return Ok(None);
        };
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let message = ToNode::Save {
            id,
            sandbox: run.conversation_id,
            previous: run.sandbox_image.clone(),
        };
        match process.request(id, message).await? {
            Response::Saved(Saved::Image(image)) => Ok(Some(image)),
            Response::Saved(Saved::Unchanged) => Ok(None),
            Response::Saved(Saved::Error(error)) => Err(anyhow!(error)),
            Response::Exec(_) => bail!("the sandbox service answered a save with a command result"),
        }
    }

    pub fn close(&self, sandbox: i64) {
        if let Some(process) = self.running() {
            process.send(ToNode::Close { sandbox });
        }
    }

    fn running(&self) -> Option<Arc<Process>> {
        self.process
            .lock()
            .as_ref()
            .filter(|process| process.is_running())
            .cloned()
    }

    fn process(&self, app: &Arc<App>) -> Result<Arc<Process>> {
        let mut slot = self.process.lock();
        if let Some(process) = slot.as_ref().filter(|process| process.is_running()) {
            return Ok(process.clone());
        }
        let process = Process::spawn(&self.entry, self.modal.as_ref(), app)?;
        *slot = Some(process.clone());
        Ok(process)
    }
}

async fn just_bash(run: &Run) -> Result<Backend> {
    let data_dir = &run.app.config.data_dir;
    let workspace = workspaces_dir(data_dir).join(run.conversation_id.to_string());
    let repos = repos_dir(data_dir);
    let prepared = (workspace.clone(), repos.clone());
    tokio::task::spawn_blocking(move || {
        std::fs::create_dir_all(&prepared.0)
            .and_then(|()| std::fs::create_dir_all(&prepared.1))
            .context("failed to create sandbox directories")?;
        touch(&prepared.0)
    })
    .await
    .context("sandbox preparation task failed")??;
    Ok(Backend::JustBash {
        workspace,
        repos,
        team: run.team,
    })
}

async fn decode_image(vision: bool, image: NodeImage) -> Result<ImageData> {
    if !vision {
        return Err(anyhow!(
            "view: {}: the model has no image input",
            image.name
        ));
    }
    let bytes = base64::engine::general_purpose::STANDARD
        .decode(image.base64)
        .with_context(|| format!("view: {}: invalid image data", image.name))?;
    attachments::image_from_bytes(bytes)
        .await
        .with_context(|| format!("view: {}", image.name))
}
