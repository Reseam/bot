use serde_json::{Map, Value};

use crate::config::LlmConfig;
use crate::llm::{Message, UserContent};

pub fn llm_config(base_url: String) -> LlmConfig {
    LlmConfig {
        base_url,
        api_key: "test-key".to_owned(),
        model: "test-model".to_owned(),
        context_window: 128_000,
        max_output_tokens: 16_000,
        extra_body: Map::new(),
    }
}

pub fn user(text: &str) -> Message {
    Message::User {
        content: UserContent::Text(text.to_owned()),
    }
}

pub fn sse(events: &[Value]) -> String {
    let mut body = events
        .iter()
        .map(|event| format!("data: {event}\n\n"))
        .collect::<String>();
    body.push_str("data: [DONE]\n\n");
    body
}
