use std::time::Duration;

use anyhow::{Context, Result, bail};

const MAX_OUTPUT_BYTES: usize = 50 * 1024;
const MAX_OUTPUT_LINES: usize = 2_000;

pub const DISCORD_MESSAGE_LIMIT: usize = 2_000;

pub fn parse_duration(input: &str) -> Result<Duration> {
    let duration = humantime::parse_duration(input).context("invalid duration")?;
    if duration.is_zero() {
        bail!("duration must be greater than zero");
    }
    Ok(duration)
}

pub fn truncate_chars(text: &str, limit: usize) -> String {
    if text.chars().count() <= limit {
        return text.to_owned();
    }
    text.chars()
        .take(limit.saturating_sub(1))
        .collect::<String>()
        + "…"
}

pub fn truncate_output(text: &str) -> String {
    let line_boundary = text
        .match_indices('\n')
        .nth(MAX_OUTPUT_LINES - 1)
        .map_or(text.len(), |(index, _)| index + 1);
    let mut boundary = text.len().min(MAX_OUTPUT_BYTES).min(line_boundary);
    while !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    if boundary == text.len() {
        return text.to_owned();
    }

    let omitted = &text[boundary..];
    let omitted_lines = omitted.lines().count();
    format!(
        "{}\n[truncated: omitted {omitted_lines} lines, {} bytes]",
        &text[..boundary],
        omitted.len()
    )
}

pub fn split_message(text: &str, limit: usize) -> Vec<String> {
    if text.is_empty() {
        return Vec::new();
    }
    let mut rest = text;
    let mut chunks = Vec::new();
    let mut fence: Option<String> = None;
    while !rest.is_empty() {
        let prefix = fence
            .as_ref()
            .map_or_else(String::new, |info| format!("```{info}\n"));
        let room = limit.saturating_sub(prefix.chars().count()).max(1);
        let mut split = clean_boundary(rest, room);
        let mut next_fence = scan_fence(&rest[..split], fence.as_deref());
        if next_fence.is_some()
            && prefix.chars().count() + rest[..split].chars().count() + 4 > limit
        {
            split = clean_boundary(rest, room.saturating_sub(4).max(1));
            next_fence = scan_fence(&rest[..split], fence.as_deref());
        }
        let raw = &rest[..split];
        let mut chunk = prefix;
        chunk.push_str(raw);
        if next_fence.is_some() {
            if !chunk.ends_with('\n') {
                chunk.push('\n');
            }
            chunk.push_str("```");
        }
        chunks.push(chunk);
        fence = next_fence;
        rest = &rest[split..];
    }
    chunks
}

fn clean_boundary(text: &str, limit: usize) -> usize {
    if text.chars().count() <= limit {
        return text.len();
    }
    let hard = text
        .char_indices()
        .nth(limit)
        .map_or(text.len(), |(index, _)| index);
    let candidate = &text[..hard];
    candidate
        .rfind("\n\n")
        .map(|index| index + 2)
        .or_else(|| candidate.rfind('\n').map(|index| index + 1))
        .or_else(|| candidate.rfind(' ').map(|index| index + 1))
        .filter(|boundary| *boundary > 0)
        .unwrap_or(hard)
}

fn scan_fence(text: &str, initial: Option<&str>) -> Option<String> {
    text.lines()
        .fold(initial.map(str::to_owned), |fence, line| {
            let trimmed = line.trim_start();
            if let Some(info) = trimmed.strip_prefix("```") {
                if fence.is_some() {
                    None
                } else {
                    Some(info.trim().to_owned())
                }
            } else {
                fence
            }
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn split_prefers_paragraph_then_line_then_space() {
        assert_eq!(split_message("aaaa\n\nbbbb", 9), ["aaaa\n\n", "bbbb"]);
        assert_eq!(split_message("aaaa\nbbbb", 8), ["aaaa\n", "bbbb"]);
        assert_eq!(split_message("aaaa bbbb", 8), ["aaaa ", "bbbb"]);
    }

    #[test]
    fn split_hard_boundary_preserves_utf8() {
        let chunks = split_message("éééé", 6);
        assert_eq!(chunks.concat(), "éééé");
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 6));
    }

    #[test]
    fn split_closes_and_reopens_code_fence() {
        let chunks = split_message("```rust\nlet alpha = 1;\nlet beta = 2;\n```", 25);
        assert!(chunks.len() > 1);
        assert!(chunks[0].ends_with("\n```"));
        assert!(chunks[1].starts_with("```rust\n"));
        assert!(chunks.iter().all(|chunk| chunk.chars().count() <= 25));
    }

    #[test]
    fn truncates_at_byte_limit_on_utf8_boundary() {
        let text = format!("{}éafter", "a".repeat(MAX_OUTPUT_BYTES - 1));
        let output = truncate_output(&text);
        assert!(output.contains("[truncated: omitted 1 lines, 7 bytes]"));
        assert!(output.is_char_boundary(output.len()));
    }

    #[test]
    fn truncates_at_line_limit() {
        let text = (0..2_005)
            .map(|index| format!("{index}\n"))
            .collect::<String>();
        let output = truncate_output(&text);
        assert!(output.contains("[truncated: omitted 5 lines,"));
        assert_eq!(output.matches('\n').count(), MAX_OUTPUT_LINES + 1);
    }

    #[test]
    fn leaves_short_output_unchanged() {
        assert_eq!(truncate_output("short\noutput"), "short\noutput");
    }

    #[test]
    fn duration_parser_rejects_zero_and_invalid_values() -> anyhow::Result<()> {
        assert_eq!(parse_duration("2h 30m")?, Duration::from_secs(9_000));
        assert!(parse_duration("").is_err());
        assert!(parse_duration("0s").is_err());
        assert!(parse_duration("tomorrow").is_err());
        Ok(())
    }
}
