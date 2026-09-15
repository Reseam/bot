use std::path::Path;

use anyhow::{Context, Result, bail};
use futures::StreamExt;
use poise::serenity_prelude::Attachment;
use reqwest::Client;

use crate::text::truncate_output;
use crate::tools::ImageData;

mod convert;

use convert::{load_docx, load_image, load_pdf, load_text, resize_image};

const MAX_IMAGE_DOWNLOAD: u32 = 20 * 1024 * 1024;
const MAX_DOCUMENT_DOWNLOAD: u32 = 25 * 1024 * 1024;
const MAX_TEXT_DOWNLOAD: u32 = 2 * 1024 * 1024;
const UTF8_SNIFF_BYTES: usize = 8 * 1024;

pub enum Loaded {
    Image(ImageData),
    Text { name: String, text: String },
    Unsupported { name: String, reason: String },
}

pub async fn load(http: &Client, attachment: &Attachment, vision: bool) -> Loaded {
    match load_inner(http, attachment, vision).await {
        Ok(loaded) => loaded,
        Err(error) => Loaded::Unsupported {
            name: attachment.filename.clone(),
            reason: format!("{error:#}"),
        },
    }
}

pub async fn text(http: &Client, attachment: &Attachment) -> Result<String> {
    if classify(attachment) == Kind::Image {
        bail!("it is an image; save it with --output and look at it with view");
    }
    match load_inner(http, attachment, false).await? {
        Loaded::Text { text, .. } => Ok(text),
        Loaded::Image(_) | Loaded::Unsupported { .. } => bail!("unsupported file type"),
    }
}

pub async fn download_original(http: &Client, attachment: &Attachment) -> Result<Vec<u8>> {
    if attachment.size > MAX_DOCUMENT_DOWNLOAD {
        bail!(
            "file is larger than {} MiB",
            MAX_DOCUMENT_DOWNLOAD / 1024 / 1024
        );
    }
    download(http, &attachment.url, MAX_DOCUMENT_DOWNLOAD).await
}

pub async fn image_from_bytes(bytes: Vec<u8>) -> Result<ImageData> {
    tokio::task::spawn_blocking(move || resize_image(&bytes))
        .await
        .context("image processing task failed")?
}

async fn load_inner(http: &Client, attachment: &Attachment, vision: bool) -> Result<Loaded> {
    let kind = classify(attachment);
    if kind == Kind::Image && !vision {
        bail!("the model has no image input");
    }
    let limit = match kind {
        Kind::Image => MAX_IMAGE_DOWNLOAD,
        Kind::Pdf | Kind::Docx => MAX_DOCUMENT_DOWNLOAD,
        Kind::Text | Kind::Unknown => MAX_TEXT_DOWNLOAD,
    };
    if attachment.size > limit {
        bail!("file is larger than {} MiB", limit / 1024 / 1024);
    }

    let bytes = download(http, &attachment.url, limit).await?;
    let name = attachment.filename.clone();
    match kind {
        Kind::Image => Ok(Loaded::Image(load_image(bytes, attachment).await?)),
        Kind::Pdf => Ok(Loaded::Text {
            name,
            text: truncate_output(&load_pdf(bytes).await?),
        }),
        Kind::Docx => Ok(Loaded::Text {
            name,
            text: truncate_output(&load_docx(bytes).await?),
        }),
        Kind::Text => Ok(Loaded::Text {
            name,
            text: truncate_output(&load_text(bytes)?),
        }),
        Kind::Unknown if looks_like_text(&bytes) => Ok(Loaded::Text {
            name,
            text: truncate_output(&load_text(bytes)?),
        }),
        Kind::Unknown => bail!("unsupported file type"),
    }
}

#[derive(Clone, Copy, Eq, PartialEq)]
enum Kind {
    Image,
    Pdf,
    Docx,
    Text,
    Unknown,
}

fn classify(attachment: &Attachment) -> Kind {
    let content_type = attachment
        .content_type
        .as_deref()
        .unwrap_or_default()
        .split(';')
        .next()
        .unwrap_or_default()
        .to_ascii_lowercase();
    let extension = Path::new(&attachment.filename)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    if matches!(
        content_type.as_str(),
        "image/png" | "image/jpeg" | "image/webp" | "image/gif"
    ) || matches!(extension.as_str(), "png" | "jpg" | "jpeg" | "webp" | "gif")
    {
        Kind::Image
    } else if content_type == "application/pdf" || extension == "pdf" {
        Kind::Pdf
    } else if content_type
        == "application/vnd.openxmlformats-officedocument.wordprocessingml.document"
        || extension == "docx"
    {
        Kind::Docx
    } else if is_text_type(&content_type) || is_text_extension(&extension) {
        Kind::Text
    } else {
        Kind::Unknown
    }
}

fn is_text_type(content_type: &str) -> bool {
    content_type.starts_with("text/")
        || matches!(
            content_type,
            "application/json"
                | "application/ld+json"
                | "application/xml"
                | "application/yaml"
                | "application/toml"
                | "application/javascript"
                | "application/x-javascript"
        )
        || content_type.ends_with("+json")
        || content_type.ends_with("+xml")
}

fn is_text_extension(extension: &str) -> bool {
    matches!(
        extension,
        "txt"
            | "md"
            | "rst"
            | "json"
            | "jsonl"
            | "xml"
            | "yaml"
            | "yml"
            | "toml"
            | "js"
            | "jsx"
            | "ts"
            | "tsx"
            | "rs"
            | "py"
            | "kt"
            | "kts"
            | "java"
            | "c"
            | "h"
            | "cc"
            | "cpp"
            | "hpp"
            | "cs"
            | "go"
            | "rb"
            | "php"
            | "swift"
            | "sh"
            | "bash"
            | "zsh"
            | "fish"
            | "sql"
            | "html"
            | "css"
            | "scss"
            | "vue"
            | "svelte"
            | "gradle"
            | "properties"
            | "ini"
            | "conf"
            | "cfg"
            | "env"
            | "log"
            | "csv"
            | "tsv"
            | "dockerfile"
            | "gitignore"
    )
}

async fn download(http: &Client, url: &str, limit: u32) -> Result<Vec<u8>> {
    let response = http.get(url).send().await.context("download failed")?;
    if !response.status().is_success() {
        bail!("download failed with status {}", response.status());
    }
    if response
        .content_length()
        .is_some_and(|size| size > u64::from(limit))
    {
        bail!("file is larger than {} MiB", limit / 1024 / 1024);
    }
    let mut stream = response.bytes_stream();
    let mut bytes = Vec::new();
    while let Some(chunk) = stream.next().await {
        let chunk = chunk.context("download failed")?;
        if bytes.len().saturating_add(chunk.len()) > limit as usize {
            bail!("file is larger than {} MiB", limit / 1024 / 1024);
        }
        bytes.extend_from_slice(&chunk);
    }
    Ok(bytes)
}

fn looks_like_text(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(UTF8_SNIFF_BYTES)];
    !sample.contains(&0) && std::str::from_utf8(sample).is_ok()
}

#[cfg(test)]
mod tests;
