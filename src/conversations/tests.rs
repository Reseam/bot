use super::*;
use crate::llm::anthropic::ContentBlock;
use crate::llm::{AssistantMessage, ImageUrl};
use crate::test_support::user;

fn replayed(text: &str) -> Message {
    Message::Assistant(AssistantMessage {
        content: Some(text.to_owned()),
        anthropic_content: vec![ContentBlock::Text {
            text: text.to_owned(),
        }],
        ..AssistantMessage::default()
    })
}

#[test]
fn stripped_images_drop_the_replay_bound_to_them() {
    let image = Message::User {
        content: UserContent::Parts(vec![
            ContentPart::Text {
                text: "look".to_owned(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "data:image/png;base64,AA==".to_owned(),
                },
            },
        ]),
    };

    let stored = strip_images(&[user("start"), replayed("before"), image, replayed("after")]);

    assert_eq!(stored[1], replayed("before"));
    assert!(matches!(
        &stored[2],
        Message::User { content: UserContent::Parts(parts) }
            if !parts.iter().any(|part| matches!(part, ContentPart::ImageUrl { .. }))
    ));
    assert!(matches!(
        &stored[3],
        Message::Assistant(message)
            if message.anthropic_content.is_empty() && message.content.as_deref() == Some("after")
    ));
}
