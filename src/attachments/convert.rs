use std::io::{Cursor, Read};
use std::path::Path;

use anyhow::{Context, Result, bail};
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, GenericImageView, ImageFormat};
use poise::serenity_prelude::Attachment;
use quick_xml::Reader;
use quick_xml::events::Event;
use zip::ZipArchive;

use crate::tools::ImageData;

const MAX_DOCX_XML_BYTES: u64 = 16 * 1024 * 1024;
const IMAGE_PASSTHROUGH_BYTES: u32 = 4 * 1024 * 1024;
const MAX_IMAGE_SIDE: u32 = 2_048;

pub(super) async fn load_image(bytes: Vec<u8>, attachment: &Attachment) -> Result<ImageData> {
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

pub(super) fn resize_image(bytes: &[u8]) -> Result<ImageData> {
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

pub(super) async fn load_pdf(bytes: Vec<u8>) -> Result<String> {
    let text = tokio::task::spawn_blocking(move || pdf_extract::extract_text_from_mem(&bytes))
        .await
        .context("PDF processing task failed")?
        .context("could not read PDF")?;
    if text.trim().is_empty() {
        bail!("no extractable text, probably scanned");
    }
    Ok(text)
}

pub(super) async fn load_docx(bytes: Vec<u8>) -> Result<String> {
    let text = tokio::task::spawn_blocking(move || extract_docx(&bytes))
        .await
        .context("DOCX processing task failed")??;
    if text.trim().is_empty() {
        bail!("document contains no text");
    }
    Ok(text)
}

pub(super) fn extract_docx(bytes: &[u8]) -> Result<String> {
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

pub(super) fn load_text(bytes: Vec<u8>) -> Result<String> {
    String::from_utf8(bytes).context("text file is not valid UTF-8")
}
