use super::*;

#[test]
fn duration_parser_accepts_compound_values_and_rejects_edges() -> Result<()> {
    assert_eq!(
        parse_duration("2h 30m")?,
        std::time::Duration::from_secs(9_000)
    );
    assert!(parse_duration("").is_err());
    assert!(parse_duration("0s").is_err());
    assert!(parse_duration("tomorrow").is_err());
    Ok(())
}

#[test]
fn history_budget_keeps_newest_complete_messages() {
    let messages = vec![
        "oldest".to_owned(),
        "middle".to_owned(),
        "newest".to_owned(),
    ];
    assert_eq!(
        trim_history(&messages, 16),
        TrimmedHistory {
            text: "middle\n\nnewest".to_owned(),
            dropped: 1,
            partial: false,
        }
    );
}

#[test]
fn history_budget_keeps_suffix_of_oversized_newest_message() {
    let messages = vec!["old".to_owned(), "abcdefgh".to_owned()];
    assert_eq!(
        trim_history(&messages, 4),
        TrimmedHistory {
            text: "efgh".to_owned(),
            dropped: 1,
            partial: true,
        }
    );
}
