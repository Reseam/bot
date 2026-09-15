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
fn duration_parser_rejects_zero_and_invalid_values() -> anyhow::Result<()> {
    assert_eq!(parse_duration("2h 30m")?, Duration::from_secs(9_000));
    assert!(parse_duration("").is_err());
    assert!(parse_duration("0s").is_err());
    assert!(parse_duration("tomorrow").is_err());
    Ok(())
}
