use super::history_content;

#[test]
fn strips_only_trailing_renderer_footers_from_bot_messages() {
    let bot_message = "Answer\n\n-# Used repo_read\n-# Compacted earlier context";
    assert_eq!(history_content(bot_message, true), "Answer");
    assert_eq!(history_content(bot_message, false), bot_message);
    assert_eq!(
        history_content("Answer\n-# Used repo_read\nMore answer", true),
        "Answer\n-# Used repo_read\nMore answer"
    );
}
