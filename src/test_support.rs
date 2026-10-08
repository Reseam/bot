use std::collections::BTreeMap;
use std::sync::Arc;

use serde_json::{Map, Value};

use crate::config::{LlmConfig, ModelConfig, ProviderConfig, ProviderKind};
use crate::llm::{Llm, Message, Model, UserContent};

pub fn test_model(kind: ProviderKind, base_url: String) -> Arc<Model> {
    let config = LlmConfig {
        default_model: "test/test-model".to_owned(),
        providers: BTreeMap::from([(
            "test".to_owned(),
            ProviderConfig {
                kind,
                base_url,
                api_key: "test-key".to_owned(),
                models: vec![ModelConfig {
                    id: "test-model".to_owned(),
                    context_window: 128_000,
                    max_output_tokens: 16_000,
                    vision: true,
                    extra_body: Map::new(),
                }],
            },
        )]),
    };
    Llm::new(&config)
        .expect("the test LLM client builds")
        .default_model()
        .clone()
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

pub fn anthropic_sse(events: &[Value]) -> String {
    events
        .iter()
        .map(|event| {
            format!(
                "event: {}\ndata: {event}\n\n",
                event["type"].as_str().unwrap_or_default()
            )
        })
        .collect()
}
