use std::io::{Cursor, Write};

use anyhow::Result;
use base64::Engine;
use image::{DynamicImage, GenericImageView, ImageBuffer, ImageFormat, Rgb};
use zip::write::SimpleFileOptions;

use super::convert::{extract_docx, resize_image};

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
