use std::collections::HashSet;

use anyhow::{Context, Result, bail};

use super::MAX_ENTRIES;

const HEADER_SIGNATURE: &[u8] = b"PK\x01\x02";
const HEADER_BYTES: usize = 46;

pub(super) fn validate(bytes: &[u8], start: u64, indexed_entries: usize) -> Result<()> {
    let start = usize::try_from(start).context("ZIP directory offset is too large")?;
    let mut remaining = bytes.get(start..).context("invalid ZIP directory offset")?;
    let mut names = HashSet::new();
    let mut count = 0;
    // ZipArchive indexes by filename and silently drops duplicate central-directory records.
    while remaining.starts_with(HEADER_SIGNATURE) {
        let header = remaining
            .get(..HEADER_BYTES)
            .context("truncated ZIP directory header")?;
        let name_len = usize::from(u16::from_le_bytes([header[28], header[29]]));
        let extra_len = usize::from(u16::from_le_bytes([header[30], header[31]]));
        let comment_len = usize::from(u16::from_le_bytes([header[32], header[33]]));
        let name = remaining
            .get(HEADER_BYTES..HEADER_BYTES + name_len)
            .context("truncated ZIP directory filename")?;
        count += 1;
        if count > MAX_ENTRIES {
            bail!("archive exceeds {MAX_ENTRIES} entries");
        }
        if !names.insert(name) {
            bail!(
                "duplicate ZIP directory filename {:?}",
                String::from_utf8_lossy(name)
            );
        }
        remaining = remaining
            .get(HEADER_BYTES + name_len + extra_len + comment_len..)
            .context("truncated ZIP directory entry")?;
    }
    if count != indexed_entries {
        bail!("ZIP directory entry count does not match its index");
    }
    Ok(())
}
