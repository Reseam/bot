use std::io::{Cursor, Read};
use std::path::Path;

use anyhow::{Context, Result, bail};
use futures::StreamExt;
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, GenericImageView, ImageFormat};
use poise::serenity_prelude::Attachment;
use quick_xml::Reader;
use quick_xml::events::Event;
use reqwest::Client;
use zip::ZipArchive;

use crate::text::truncate_output;
use crate::tools::ImageData;

const MAX_IMAGE_DOWNLOAD: u32 = 20 * 1024 * 1024;
const MAX_DOCUMENT_DOWNLOAD: u32 = 25 * 1024 * 1024;
const MAX_TEXT_DOWNLOAD: u32 = 2 * 1024 * 1024;
const MAX_DOCX_XML_BYTES: u64 = 16 * 1024 * 1024;
const IMAGE_PASSTHROUGH_BYTES: u32 = 4 * 1024 * 1024;
const MAX_IMAGE_SIDE: u32 = 2_048;
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

async fn load_image(bytes: Vec<u8>, attachment: &Attachment) -> Result<ImageData> {
    let extension = Path::new(&attachment.filename)
        .extension()
        .and_then(|value| value.to_str())
        .unwrap_or_default();
    let mime = attachment.content_type.as_deref().unwrap_or_default();
    let pass_format = if mime == "image/png" || extension.eq_ignore_ascii_case("png") {
        Some("image/png")
    } else if mime == "image/jpeg"
        || extension.eq_ignore_ascii_case("jpg")
        || extension.eq_ignore_ascii_case("jpeg")
    {
        Some("image/jpeg")
    } else {
        None
    };
    if let Some(pass_format) = pass_format
        && attachment.size <= IMAGE_PASSTHROUGH_BYTES
        && attachment
            .dimensions()
            .is_some_and(|(width, height)| width.max(height) <= MAX_IMAGE_SIDE)
    {
        return Ok(ImageData::from_bytes(pass_format, &bytes));
    }

    tokio::task::spawn_blocking(move || resize_image(&bytes))
        .await
        .context("image processing task failed")?
}

fn resize_image(bytes: &[u8]) -> Result<ImageData> {
    let image = image::load_from_memory(bytes).context("invalid image")?;
    let (width, height) = image.dimensions();
    let resized = if width.max(height) > MAX_IMAGE_SIDE {
        image.resize(
            MAX_IMAGE_SIDE,
            MAX_IMAGE_SIDE,
            image::imageops::FilterType::Lanczos3,
        )
    } else {
        image
    };
    encode_image(resized)
}

fn encode_image(image: DynamicImage) -> Result<ImageData> {
    let mut output = Vec::new();
    if image.color().has_alpha() {
        image
            .write_to(&mut Cursor::new(&mut output), ImageFormat::Png)
            .context("image encoding failed")?;
        Ok(ImageData::from_bytes("image/png", &output))
    } else {
        JpegEncoder::new_with_quality(&mut output, 85)
            .encode_image(&image)
            .context("image encoding failed")?;
        Ok(ImageData::from_bytes("image/jpeg", &output))
    }
}

async fn load_pdf(bytes: Vec<u8>) -> Result<String> {
    let text = tokio::task::spawn_blocking(move || pdf_extract::extract_text_from_mem(&bytes))
        .await
        .context("PDF processing task failed")?
        .context("could not read PDF")?;
    if text.trim().is_empty() {
        bail!("no extractable text, probably scanned");
    }
    Ok(text)
}

async fn load_docx(bytes: Vec<u8>) -> Result<String> {
    let text = tokio::task::spawn_blocking(move || extract_docx(&bytes))
        .await
        .context("DOCX processing task failed")??;
    if text.trim().is_empty() {
        bail!("document contains no text");
    }
    Ok(text)
}

fn extract_docx(bytes: &[u8]) -> Result<String> {
    let mut archive = ZipArchive::new(Cursor::new(bytes)).context("invalid DOCX archive")?;
    let document = archive
        .by_name("word/document.xml")
        .context("DOCX has no document body")?;
    let mut limited = document.take(MAX_DOCX_XML_BYTES + 1);
    let mut xml = String::new();
    limited
        .read_to_string(&mut xml)
        .context("could not read DOCX body")?;
    if xml.len() as u64 > MAX_DOCX_XML_BYTES {
        bail!("DOCX document body is larger than 16 MiB");
    }
    let mut reader = Reader::from_str(&xml);
    let mut output = String::new();
    let mut in_text = false;
    loop {
        match reader.read_event() {
            Ok(Event::Start(tag)) => match tag.local_name().as_ref() {
                "t" => in_text = true,
                "tab" => output.push('\t'),
                "br" => output.push('\n'),
                _ => {}
            },
            Ok(Event::Empty(tag)) => match tag.local_name().as_ref() {
                "tab" => output.push('\t'),
                "br" => output.push('\n'),
                _ => {}
            },
            Ok(Event::Text(text)) if in_text => output.push_str(
                &quick_xml::escape::unescape(text.as_ref()).context("invalid DOCX text")?,
            ),
            Ok(Event::End(tag)) => match tag.local_name().as_ref() {
                "t" => in_text = false,
                "p" => output.push('\n'),
                _ => {}
            },
            Ok(Event::Eof) => break,
            Err(error) => return Err(error).context("invalid DOCX XML"),
            _ => {}
        }
    }
    Ok(output)
}

fn load_text(bytes: Vec<u8>) -> Result<String> {
    String::from_utf8(bytes).context("text file is not valid UTF-8")
}

fn looks_like_text(bytes: &[u8]) -> bool {
    let sample = &bytes[..bytes.len().min(UTF8_SNIFF_BYTES)];
    !sample.contains(&0) && std::str::from_utf8(sample).is_ok()
}

#[cfg(test)]
mod tests {
    use std::io::Write;

    use base64::Engine;
    use image::{ImageBuffer, Rgb};
    use zip::write::SimpleFileOptions;

    use super::*;

    #[test]
    fn extracts_docx_paragraphs_tabs_and_breaks() -> Result<()> {
        let xml = r#"<w:document xmlns:w="x"><w:body><w:p><w:r><w:t>Hello</w:t><w:tab/><w:t>world</w:t><w:br/><w:t>again</w:t></w:r></w:p><w:p><w:r><w:t>Next</w:t></w:r></w:p></w:body></w:document>"#;
        let mut bytes = Vec::new();
        {
            let mut archive = zip::ZipWriter::new(Cursor::new(&mut bytes));
            archive.start_file("word/document.xml", SimpleFileOptions::default())?;
            archive.write_all(xml.as_bytes())?;
            archive.finish()?;
        }
        assert_eq!(extract_docx(&bytes)?, "Hello\tworld\nagain\nNext\n");
        Ok(())
    }

    #[test]
    fn detects_utf8_text_without_nul() {
        assert!(looks_like_text("hello é".as_bytes()));
        assert!(!looks_like_text(b"hello\0world"));
        assert!(!looks_like_text(&[0xff, 0xfe]));
    }

    #[test]
    fn resizes_wide_image_to_limit() -> Result<()> {
        let image = DynamicImage::ImageRgb8(ImageBuffer::from_pixel(4000, 1000, Rgb([1, 2, 3])));
        let mut source = Vec::new();
        image.write_to(&mut Cursor::new(&mut source), ImageFormat::Png)?;
        let encoded = resize_image(&source)?;
        let bytes = base64::engine::general_purpose::STANDARD.decode(encoded.base64_data)?;
        let decoded = image::load_from_memory(&bytes)?;
        assert_eq!(decoded.dimensions(), (2048, 512));
        Ok(())
    }
}
