use std::collections::HashMap;
use std::path::Path;
use std::process::Stdio;
use std::sync::{Arc, Weak};

use anyhow::{Context, Result, anyhow};
use parking_lot::Mutex;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::process::{Child, ChildStdin, ChildStdout, Command};
use tokio::sync::{mpsc, oneshot};
use tracing::{error, warn};

use super::files;
use super::protocol::{ExecResult, FromNode, ReplyResult, Saved, ToNode};
use crate::App;
use crate::cli;
use crate::config::ModalConfig;

pub(super) enum Response {
    Exec(ExecResult),
    Saved(Saved),
}

pub(super) struct Process {
    outgoing: mpsc::UnboundedSender<ToNode>,
    pending: Mutex<Option<HashMap<u64, oneshot::Sender<Response>>>>,
    pub(super) file_reads: Mutex<HashMap<u64, mpsc::UnboundedSender<files::FileResult>>>,
    _child: Child,
}

struct Pending {
    process: Arc<Process>,
    id: u64,
    finished: bool,
}

impl Drop for Pending {
    fn drop(&mut self) {
        if !self.finished {
            if let Some(pending) = self.process.pending.lock().as_mut() {
                pending.remove(&self.id);
            }
            self.process.send(ToNode::Cancel { id: self.id });
        }
    }
}

impl Process {
    pub(super) fn spawn(
        entry: &Path,
        modal: Option<&ModalConfig>,
        app: &Arc<App>,
    ) -> Result<Arc<Self>> {
        let mut child = Command::new("node")
            .arg(entry)
            .stdin(Stdio::piped())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            .kill_on_drop(true)
            .spawn()
            .with_context(|| format!("failed to start sandbox service {}", entry.display()))?;
        let stdin = child.stdin.take().context("sandbox stdin is unavailable")?;
        let stdout = child
            .stdout
            .take()
            .context("sandbox stdout is unavailable")?;
        let (outgoing, receiver) = mpsc::unbounded_channel();
        let process = Arc::new(Self {
            outgoing,
            pending: Mutex::new(Some(HashMap::new())),
            file_reads: Mutex::new(HashMap::new()),
            _child: child,
        });
        process.send(ToNode::Configure {
            modal: modal.cloned(),
        });
        tokio::spawn(write_messages(stdin, receiver));
        tokio::spawn(read_messages(
            Arc::downgrade(app),
            Arc::downgrade(&process),
            stdout,
        ));
        Ok(process)
    }

    pub(super) fn is_running(&self) -> bool {
        self.pending.lock().is_some()
    }

    pub(super) fn send(&self, message: ToNode) {
        if self.outgoing.send(message).is_err() {
            warn!("sandbox service is not running");
        }
    }

    pub(super) async fn request(self: &Arc<Self>, id: u64, message: ToNode) -> Result<Response> {
        let (sender, receiver) = oneshot::channel();
        self.pending
            .lock()
            .as_mut()
            .context("the sandbox service is not running")?
            .insert(id, sender);
        let mut pending = Pending {
            process: self.clone(),
            id,
            finished: false,
        };
        self.send(message);
        let response = receiver
            .await
            .map_err(|_| anyhow!("the sandbox service stopped before replying"))?;
        pending.finished = true;
        Ok(response)
    }

    fn respond(&self, id: u64, response: Response) {
        let sender = self
            .pending
            .lock()
            .as_mut()
            .and_then(|pending| pending.remove(&id));
        if let Some(sender) = sender {
            let _ = sender.send(response);
        }
    }
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
            FromNode::FileResult { id, result } => {
                if let Some(sender) = process.file_reads.lock().get(&id) {
                    let _ = sender.send(result);
                }
            }
            FromNode::ExecResult(result) => process.respond(result.id, Response::Exec(result)),
            FromNode::Saved { id, result } => process.respond(id, Response::Saved(result)),
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
                            let mut files = files::Files::new(process.clone(), id);
                            tokio::select! {
                                output = cli::run(&run, command, args, stdin, &mut files) => ReplyResult::Command(output),
                                () = run.cancel.cancelled() => ReplyResult::Error("cancelled".to_owned()),
                            }
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
        process.file_reads.lock().clear();
    }
    warn!("sandbox service exited");
}
