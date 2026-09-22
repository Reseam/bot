use std::sync::Arc;

use anyhow::{Context, Result, bail};
use base64::Engine;
use serde::{Deserialize, Serialize};
use tokio::sync::mpsc;

use super::{Process, ToNode};

#[derive(Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FileResult {
    Base64(String),
    Entries(Vec<Entry>),
    Written,
    Error(String),
}

#[derive(Deserialize)]
pub struct Entry {
    pub path: String,
    pub name: String,
    pub directory: bool,
}

#[derive(Serialize)]
pub struct TreeEntry {
    pub name: String,
    pub base64: Option<String>,
}

pub struct Files {
    process: Arc<Process>,
    id: u64,
    receiver: mpsc::UnboundedReceiver<FileResult>,
}

impl Files {
    pub(super) fn new(process: Arc<Process>, id: u64) -> Self {
        let (sender, receiver) = mpsc::unbounded_channel();
        process.file_reads.lock().insert(id, sender);
        Self {
            process,
            id,
            receiver,
        }
    }

    pub async fn walk(
        &mut self,
        paths: Vec<String>,
        output: String,
        recursive: bool,
        exclude: Vec<String>,
        max_entries: usize,
    ) -> Result<Vec<Entry>> {
        self.process.send(ToNode::WalkFiles {
            id: self.id,
            paths,
            output,
            recursive,
            exclude,
            max_entries,
        });
        match self
            .receiver
            .recv()
            .await
            .context("sandbox file reader stopped")?
        {
            FileResult::Entries(entries) => Ok(entries),
            FileResult::Error(error) => bail!("failed to walk sandbox files: {error}"),
            _ => bail!("unexpected sandbox walk response"),
        }
    }

    pub async fn write_tree(&mut self, path: String, entries: Vec<TreeEntry>) -> Result<()> {
        self.process.send(ToNode::WriteTree {
            id: self.id,
            path,
            entries,
        });
        match self
            .receiver
            .recv()
            .await
            .context("sandbox file writer stopped")?
        {
            FileResult::Written => Ok(()),
            FileResult::Error(error) => bail!("failed to extract sandbox files: {error}"),
            _ => bail!("unexpected sandbox write response"),
        }
    }

    pub async fn read(&mut self, path: &str, max_bytes: usize) -> Result<Vec<u8>> {
        self.process.send(ToNode::ReadFile {
            id: self.id,
            path: path.to_owned(),
            max_bytes,
        });
        let result = self
            .receiver
            .recv()
            .await
            .context("sandbox file reader stopped")?;
        let encoded = match result {
            FileResult::Base64(encoded) => encoded,
            FileResult::Error(error) => bail!("failed to read {path}: {error}"),
            _ => bail!("unexpected sandbox file response"),
        };
        let bytes = base64::engine::general_purpose::STANDARD
            .decode(encoded)
            .with_context(|| format!("invalid file data for {path}"))?;
        if bytes.len() > max_bytes {
            bail!("{path} exceeds the {max_bytes} byte upload limit");
        }
        Ok(bytes)
    }
}

impl Drop for Files {
    fn drop(&mut self) {
        self.process.file_reads.lock().remove(&self.id);
    }
}
