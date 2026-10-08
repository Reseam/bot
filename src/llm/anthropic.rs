use std::borrow::Cow;

use reqwest::RequestBuilder;
use serde::{Deserialize, Serialize};
use serde_json::{Map, Value};

use super::{AssistantMessage, ContentPart, Message, Prompt, Provider, ToolChoice, UserContent};
use crate::config::ModelConfig;

mod stream;

pub(super) use stream::parse_stream;

const API_VERSION: &str = "2023-06-01";
// Turns of one run land seconds apart, but replies and long tool calls often come minutes later.
const CACHE_TTL: &str = "1h";

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentBlock {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
    ToolUse {
        id: String,
        name: String,
        input: Value,
    },
}

#[derive(Serialize)]
struct RequestBody<'a> {
    model: &'a str,
    max_tokens: u32,
    system: [SystemBlock<'a>; 1],
    messages: Vec<WireMessage<'a>>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Tool<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<WireToolChoice>,
    stream: bool,
    cache_control: CacheControl,
    #[serde(flatten)]
    extra_body: &'a Map<String, Value>,
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum SystemBlock<'a> {
    Text {
        text: &'a str,
        cache_control: CacheControl,
    },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum CacheControl {
    Ephemeral { ttl: &'static str },
}

#[derive(Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum WireToolChoice {
    None,
}

#[derive(Serialize)]
struct Tool<'a> {
    name: &'a str,
    description: &'a str,
    input_schema: &'a Value,
}

#[derive(Debug, PartialEq, Serialize)]
struct WireMessage<'a> {
    role: Role,
    content: Vec<Block<'a>>,
}

#[derive(Clone, Copy, Debug, PartialEq, Serialize)]
#[serde(rename_all = "lowercase")]
enum Role {
    User,
    Assistant,
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Block<'a> {
    Text {
        text: &'a str,
    },
    Image {
        source: ImageSource<'a>,
    },
    ToolUse {
        id: Cow<'a, str>,
        name: &'a str,
        input: Value,
    },
    ToolResult {
        tool_use_id: Cow<'a, str>,
        content: &'a str,
    },
    #[serde(untagged)]
    Replay(&'a ContentBlock),
}

#[derive(Debug, PartialEq, Serialize)]
#[serde(tag = "type", rename_all = "lowercase")]
enum ImageSource<'a> {
    Base64 { media_type: &'a str, data: &'a str },
    Url { url: &'a str },
}

pub(super) fn request(
    provider: &Provider,
    model: &ModelConfig,
    prompt: &Prompt<'_>,
) -> RequestBuilder {
    provider
        .http
        .post(format!("{}/v1/messages", provider.base_url))
        .header("x-api-key", &provider.api_key)
        .header("anthropic-version", API_VERSION)
        .json(&RequestBody {
            model: &model.id,
            max_tokens: model.max_output_tokens,
            system: [SystemBlock::Text {
                text: prompt.system,
                cache_control: CacheControl::Ephemeral { ttl: CACHE_TTL },
            }],
            messages: wire_messages(prompt.messages),
            tools: prompt
                .tools
                .iter()
                .map(|tool| Tool {
                    name: &tool.name,
                    description: &tool.description,
                    input_schema: &tool.parameters,
                })
                .collect(),
            tool_choice: match prompt.tool_choice {
                ToolChoice::Auto => None,
                ToolChoice::None => Some(WireToolChoice::None),
            },
            stream: true,
            cache_control: CacheControl::Ephemeral { ttl: CACHE_TTL },
            extra_body: &model.extra_body,
        })
}

fn wire_messages(transcript: &[Message]) -> Vec<WireMessage<'_>> {
    let mut messages: Vec<WireMessage<'_>> = Vec::new();
    for message in transcript {
        let (role, content) = match message {
            Message::User { content } => (Role::User, user_blocks(content)),
            Message::Tool {
                tool_call_id,
                content,
            } => (
                Role::User,
                vec![Block::ToolResult {
                    tool_use_id: tool_id(tool_call_id),
                    content,
                }],
            ),
            Message::Assistant(message) => (Role::Assistant, assistant_blocks(message)),
        };
        match messages.last_mut() {
            Some(last) if last.role == role => last.content.extend(content),
            _ if content.is_empty() => {}
            _ => messages.push(WireMessage { role, content }),
        }
    }
    messages
}

fn user_blocks(content: &UserContent) -> Vec<Block<'_>> {
    match content {
        UserContent::Text(text) => text_block(text).into_iter().collect(),
        UserContent::Parts(parts) => parts
            .iter()
            .filter_map(|part| match part {
                ContentPart::Text { text } => text_block(text),
                ContentPart::ImageUrl { image_url } => Some(Block::Image {
                    source: image_source(&image_url.url),
                }),
            })
            .collect(),
    }
}

fn assistant_blocks(message: &AssistantMessage) -> Vec<Block<'_>> {
    if !message.anthropic_content.is_empty() {
        return message
            .anthropic_content
            .iter()
            .map(Block::Replay)
            .collect();
    }
    message
        .content
        .as_deref()
        .and_then(text_block)
        .into_iter()
        .chain(message.tool_calls.iter().map(|call| Block::ToolUse {
            id: tool_id(&call.id),
            name: &call.function.name,
            input: tool_input(&call.function.arguments),
        }))
        .collect()
}

fn text_block(text: &str) -> Option<Block<'_>> {
    (!text.trim().is_empty()).then_some(Block::Text { text })
}

fn image_source(url: &str) -> ImageSource<'_> {
    match url
        .strip_prefix("data:")
        .and_then(|data| data.split_once(";base64,"))
    {
        Some((media_type, data)) => ImageSource::Base64 { media_type, data },
        None => ImageSource::Url { url },
    }
}

fn tool_id(id: &str) -> Cow<'_, str> {
    let invalid =
        |character: char| !(character.is_ascii_alphanumeric() || "_-".contains(character));
    if id.contains(invalid) {
        Cow::Owned(id.replace(invalid, "_"))
    } else {
        Cow::Borrowed(id)
    }
}

// Calls whose arguments do not parse already carry an error result; the API still requires an object.
fn tool_input(arguments: &str) -> Value {
    Value::Object(serde_json::from_str(arguments).unwrap_or_default())
}

#[cfg(test)]
mod tests;
