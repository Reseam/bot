use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::process::Stdio;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Weak};
use std::time::{Duration, SystemTime};

use anyhow::{Context, Result, anyhow};
use base64::Engine;
use parking_lot::Mutex;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};
use tracing::{error, warn};

use crate::App;
use crate::attachments;
use crate::chat::Run;
use crate::cli::{self, BridgeCommand, CommandOutput};
use crate::text::truncate_output;
use crate::tools::{ImageData, ToolOutput};

mod http;

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
    http: http::Http,
    process: Mutex<Option<Arc<Process>>>,
    next_id: AtomicU64,
}

struct Process {
    outgoing: mpsc::UnboundedSender<ToNode>,
    pending: Mutex<Option<HashMap<u64, oneshot::Sender<ExecResult>>>>,
    _child: Child,
}

impl Process {
    fn is_running(&self) -> bool {
        self.pending.lock().is_some()
    }

    fn send(&self, message: ToNode) {
        if self.outgoing.send(message).is_err() {
            warn!("sandbox service is not running");
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum ToNode {
    Exec {
        id: u64,
        sandbox: i64,
        workspace: PathBuf,
        repos: PathBuf,
        command: String,
        timeout_ms: u128,
    },
    Cancel {
        id: u64,
    },
    Close {
        sandbox: i64,
    },
    Reply {
        id: u64,
        result: ReplyResult,
    },
}

#[derive(Serialize)]
#[serde(rename_all = "snake_case")]
enum ReplyResult {
    Command(CommandOutput),
    Fetch(http::FetchResponse),
    Error(String),
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum FromNode {
    ExecResult(ExecResult),
    Call {
        id: u64,
        sandbox: i64,
        command: BridgeCommand,
        args: Vec<String>,
        stdin: String,
    },
    Fetch {
        id: u64,
        sandbox: i64,
        #[serde(flatten)]
        request: http::FetchRequest,
    },
}

#[derive(Deserialize)]
struct ExecResult {
    id: u64,
    stdout: String,
    stderr: String,
    exit_code: i32,
    images: Vec<NodeImage>,
}

#[derive(Deserialize)]
struct NodeImage {
    name: String,
    base64: String,
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

struct PendingExec {
    process: Arc<Process>,
    id: u64,
    finished: bool,
}

impl Drop for PendingExec {
    fn drop(&mut self) {
        if !self.finished {
            if let Some(pending) = self.process.pending.lock().as_mut() {
                pending.remove(&self.id);
            }
            self.process.send(ToNode::Cancel { id: self.id });
        }
    }
}

impl Sandbox {
    pub fn new(entry: PathBuf) -> Result<Self> {
        Ok(Self {
            entry,
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

        let process = self.process(&run.app)?;
        let id = self.next_id.fetch_add(1, Ordering::Relaxed);
        let (sender, receiver) = oneshot::channel();
        process
            .pending
            .lock()
            .as_mut()
            .context("the sandbox service is not running")?
            .insert(id, sender);
        let mut pending = PendingExec {
            process: process.clone(),
            id,
            finished: false,
        };
        process.send(ToNode::Exec {
            id,
            sandbox: run.conversation_id,
            workspace,
            repos,
            command: command.to_owned(),
            timeout_ms: timeout.as_millis(),
        });
        let result = receiver
            .await
            .map_err(|_| anyhow!("the sandbox service stopped while running the command"))?;
        pending.finished = true;

        let mut stderr = result.stderr;
        let mut images = Vec::with_capacity(result.images.len());
        for image in result.images {
            match decode_image(&run.app, image).await {
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

    pub fn close(&self, sandbox: i64) {
        if let Some(process) = self.process.lock().as_ref() {
            process.send(ToNode::Close { sandbox });
        }
    }

    fn process(&self, app: &Arc<App>) -> Result<Arc<Process>> {
        let mut slot = self.process.lock();
        if let Some(process) = slot.as_ref().filter(|process| process.is_running()) {
            return Ok(process.clone());
        }
        let mut child = Command::new("node")
            .arg(&self.entry)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to start sandbox service {}", self.entry.display()))?;
        let stdin = child.stdin.take().context("sandbox stdin is unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("sandbox stdout is unavailable")?;
        let (outgoing, receiver) = mpsc::unbounded_channel();
        let process = Arc::new(Process {
            outgoing,
            pending: Mutex::new(Some(HashMap::new())),
            _child: child,
        });
        tokio::spawn(write_messages(stdin, receiver));
        tokio::spawn(read_messages(
            Arc::downgrade(app),
            Arc::downgrade(&process),
            stdout,
        ));
        *slot = Some(process.clone());
        Ok(process)
    }
}

async fn decode_image(app: &App, image: NodeImage) -> Result<ImageData> {
    if !app.config.llm.vision {
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

async fn write_messages(mut stdin: ChildStdin, mut receiver: mpsc::UnboundedReceiver<ToNode>) {
    while let Some(message) = receiver.recv().await {
        let mut line = match serde_json::to_vec(&message) {
            Ok(line) => line,
            Err(error) => {
                error!(?error, "failed to serialize sandbox message");
                continue;
            }
        };
        line.push(b'\n');
        if let Err(error) = stdin.write_all(&line).await {
            error!(?error, "failed to write to the sandbox service");
            return;
        }
    }
}

async fn read_messages(app: Weak<App>, process: Weak<Process>, stdout: ChildStdout) {
    let mut lines = BufReader::new(stdout).lines();
    loop {
        let line = match lines.next_line().await {
            Ok(Some(line)) => line,
            Ok(None) => break,
            Err(error) => {
                error!(?error, "failed to read from the sandbox service");
                break;
            }
        };
        let message = match serde_json::from_str::<FromNode>(&line) {
            Ok(message) => message,
            Err(error) => {
                error!(?error, "invalid message from the sandbox service");
                continue;
            }
        };
        let (Some(app), Some(process)) = (app.upgrade(), process.upgrade()) else {
            return;
        };
        match message {
            FromNode::ExecResult(result) => {
                let sender = process
                    .pending
                    .lock()
                    .as_mut()
                    .and_then(|pending| pending.remove(&result.id));
                if let Some(sender) = sender {
                    let _ = sender.send(result);
                }
            }
            FromNode::Call {
                id,
                sandbox,
                command,
                args,
                stdin,
            } => {
                tokio::spawn(async move {
                    let result = match app.runs.get_conversation(sandbox) {
                        Some(run) => {
                            ReplyResult::Command(cli::run(&run, command, args, stdin).await)
                        }
                        None => {
                            ReplyResult::Error("this conversation has no active run".to_owned())
                        }
                    };
                    process.send(ToNode::Reply { id, result });
                });
            }
            FromNode::Fetch {
                id,
                sandbox,
                request,
            } => {
                tokio::spawn(async move {
                    let result = match app.runs.get_conversation(sandbox) {
                        Some(run) => match app.sandbox.http.fetch(&run, request).await {
                            Ok(response) => ReplyResult::Fetch(response),
                            Err(error) => ReplyResult::Error(format!("{error:#}")),
                        },
                        None => {
                            ReplyResult::Error("this conversation has no active run".to_owned())
                        }
                    };
                    process.send(ToNode::Reply { id, result });
                });
            }
        }
    }
    if let Some(process) = process.upgrade() {
        process.pending.lock().take();
    }
    warn!("sandbox service exited");
}
