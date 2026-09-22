use std::collections::HashMap;
use std::io::{Cursor, Read, Write};

use anyhow::{Context, Result, bail};
use base64::Engine;
use zip::{CompressionMethod, ZipArchive, ZipWriter, write::SimpleFileOptions};

use super::{MAX_ARCHIVE_BYTES, MAX_BYTES, MAX_ENTRIES, directory};
use crate::cli::filename;
use crate::sandbox::files::TreeEntry;

const MAX_DEPTH: usize = 64;

pub(super) fn validate_entries<'a>(
    entries: impl IntoIterator<Item = (&'a str, bool)>,
) -> Result<()> {
    let mut names = HashMap::new();
    for (name, directory) in entries {
        if name.split('/').count() > MAX_DEPTH {
            bail!("archive paths must have at most {MAX_DEPTH} components");
        }
        for component in name.split('/') {
            filename(component).with_context(|| format!("unsafe archive path {name:?}"))?;
        }
        if names.insert(name, directory).is_some() {
            bail!("duplicate archive path {name:?}");
        }
        if names.len() > MAX_ENTRIES {
            bail!("archive exceeds {MAX_ENTRIES} entries");
        }
    }
    for name in names.keys() {
        let mut path = *name;
        while let Some((parent, _)) = path.rsplit_once('/') {
            if names.get(parent) == Some(&false) {
                bail!("file blocks archive directory {parent:?}");
            }
            path = parent;
        }
    }
    Ok(())
}

pub(super) fn compress(entries: Vec<(String, Option<Vec<u8>>)>) -> Result<Vec<u8>> {
    let mut archive = ZipWriter::new(Cursor::new(Vec::new()));
    let options = SimpleFileOptions::default().compression_method(CompressionMethod::Deflated);
    for (name, bytes) in entries {
        if let Some(bytes) = bytes {
            archive
                .start_file(&name, options)
                .with_context(|| format!("failed to create ZIP entry {name}"))?;
            archive
                .write_all(&bytes)
                .with_context(|| format!("failed to write ZIP entry {name}"))?;
        } else {
            archive
                .add_directory(&name, options)
                .with_context(|| format!("failed to create ZIP directory {name}"))?;
        }
    }
    let bytes = archive
        .finish()
        .context("failed to finish ZIP archive")?
        .into_inner();
    if bytes.len() > MAX_ARCHIVE_BYTES {
        bail!("ZIP exceeds the {MAX_ARCHIVE_BYTES} byte archive-size limit");
    }
    Ok(bytes)
}

pub(super) fn read(bytes: Vec<u8>, extract: bool) -> Result<(String, Vec<TreeEntry>)> {
    let mut archive =
        ZipArchive::new(Cursor::new(bytes.as_slice())).context("invalid ZIP archive")?;
    directory::validate(&bytes, archive.central_directory_start(), archive.len())?;
    if archive.len() > MAX_ENTRIES {
        bail!("archive exceeds {MAX_ENTRIES} entries");
    }
    let mut entries = Vec::with_capacity(archive.len());
    let mut summary = String::new();
    let mut remaining = MAX_BYTES;
    for index in 0..archive.len() {
        let mut file = archive
            .by_index(index)
            .context("failed to read ZIP entry")?;
        let directory = file.is_dir();
        let name = if directory {
            file.name().strip_suffix('/').unwrap_or(file.name())
        } else {
            file.name()
        }
        .to_owned();
        if let Some(mode) = file.unix_mode() {
            let kind = mode & 0o170000;
            if kind != 0 && kind != if directory { 0o040000 } else { 0o100000 } {
                bail!("symlinks and special files are not supported: {name:?}");
            }
        }
        let size = file.size();
        if size > remaining as u64 || (directory && size != 0) {
            bail!(
                "archive exceeds the {MAX_BYTES} byte extracted-size limit or has invalid directory data"
            );
        }
        remaining -= size as usize;
        summary.push_str(&format!(
            "{}\t{}\t{name}\n",
            if directory { "dir" } else { "file" },
            size
        ));
        let base64 = if directory {
            None
        } else if extract {
            let mut data = Vec::new();
            (&mut file)
                .take(size + 1)
                .read_to_end(&mut data)
                .with_context(|| format!("failed to decompress {name:?}"))?;
            if data.len() as u64 != size {
                bail!("ZIP entry size does not match its contents: {name:?}");
            }
            Some(base64::engine::general_purpose::STANDARD.encode(data))
        } else {
            Some(String::new())
        };
        entries.push(TreeEntry { name, base64 });
    }
    validate_entries(
        entries
            .iter()
            .map(|entry| (entry.name.as_str(), entry.base64.is_none())),
    )?;
    Ok((summary, entries))
}
