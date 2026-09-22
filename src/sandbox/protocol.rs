use std::path::PathBuf;

use serde::{Deserialize, Serialize};

use super::{files, http};
use crate::cli::{BridgeCommand, CommandOutput};

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum ToNode {
    Exec {
        id: u64,
        sandbox: i64,
        workspace: PathBuf,
        repos: PathBuf,
        team: bool,
        command: String,
        timeout_ms: u128,
    },
    WalkFiles {
        id: u64,
        paths: Vec<String>,
        output: String,
        recursive: bool,
        exclude: Vec<String>,
        max_entries: usize,
    },
    WriteTree {
        id: u64,
        path: String,
        entries: Vec<files::TreeEntry>,
    },
    ReadFile {
        id: u64,
        path: String,
        max_bytes: usize,
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
pub(super) enum ReplyResult {
    Command(CommandOutput),
    Fetch(http::FetchResponse),
    Error(String),
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub(super) enum FromNode {
    ExecResult(ExecResult),
    FileResult {
        id: u64,
        result: files::FileResult,
    },
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
pub(super) struct ExecResult {
    pub(super) id: u64,
    pub(super) stdout: String,
    pub(super) stderr: String,
    pub(super) exit_code: i32,
    pub(super) images: Vec<NodeImage>,
}

#[derive(Deserialize)]
pub(super) struct NodeImage {
    pub(super) name: String,
    pub(super) base64: String,
}
