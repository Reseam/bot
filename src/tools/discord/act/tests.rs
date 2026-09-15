use super::*;

#[test]
fn parses_unicode_and_custom_emoji_arguments() -> Result<()> {
    assert_eq!(
        parse_emoji("✅")?,
        serenity::ReactionType::Unicode("✅".to_owned())
    );
    let expected = serenity::ReactionType::Custom {
        animated: false,
        id: serenity::EmojiId::new(123),
        name: Some("reseam".to_owned()),
    };
    assert_eq!(parse_emoji("<:reseam:123>")?, expected);
    assert_eq!(parse_emoji("reseam:123")?, expected);
    assert!(parse_emoji("<:broken>").is_err());
    Ok(())
}
