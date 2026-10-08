use reqwest::RequestBuilder;
use serde::Serialize;
use serde_json::{Map, Value};

use super::{Message, Prompt, Provider, ToolCall, ToolChoice, ToolSpec, UserContent};
use crate::config::ModelConfig;

mod stream;

pub(super) use stream::parse_stream;

#[derive(Serialize)]
struct RequestBody<'a> {
    model: &'a str,
    messages: Vec<WireMessage<'a>>,
    stream: bool,
    stream_options: StreamOptions,
    max_tokens: u32,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    tools: Vec<Tool<'a>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    tool_choice: Option<&'static str>,
    #[serde(flatten)]
    extra_body: &'a Map<String, Value>,
}

#[derive(Serialize)]
#[serde(tag = "role", rename_all = "lowercase")]
enum WireMessage<'a> {
    System {
        content: &'a str,
    },
    User {
        content: &'a UserContent,
    },
    Assistant {
        content: Option<&'a str>,
        #[serde(skip_serializing_if = "<[ToolCall]>::is_empty")]
        tool_calls: &'a [ToolCall],
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning_content: Option<&'a str>,
        #[serde(skip_serializing_if = "Option::is_none")]
        reasoning: Option<&'a str>,
    },
    Tool {
        tool_call_id: &'a str,
        content: &'a str,
    },
}

#[derive(Serialize)]
struct Tool<'a> {
    #[serde(rename = "type")]
    kind: &'static str,
    function: &'a ToolSpec,
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

pub(super) fn request(
    provider: &Provider,
    model: &ModelConfig,
    prompt: &Prompt<'_>,
) -> RequestBuilder {
    let messages = std::iter::once(WireMessage::System {
        content: prompt.system,
    })
    .chain(prompt.messages.iter().map(wire_message))
    .collect();
    provider
        .http
        .post(format!("{}/chat/completions", provider.base_url))
        .bearer_auth(&provider.api_key)
        .json(&RequestBody {
            model: &model.id,
            messages,
            stream: true,
            stream_options: StreamOptions {
                include_usage: true,
            },
            max_tokens: model.max_output_tokens,
            tools: prompt
                .tools
                .iter()
                .map(|function| Tool {
                    kind: "function",
                    function,
                })
                .collect(),
            tool_choice: match prompt.tool_choice {
                ToolChoice::Auto => None,
                ToolChoice::None => Some("none"),
            },
            extra_body: &model.extra_body,
        })
}

fn wire_message(message: &Message) -> WireMessage<'_> {
    match message {
        Message::User { content } => WireMessage::User { content },
        Message::Assistant(message) => WireMessage::Assistant {
            content: message.content.as_deref(),
            tool_calls: &message.tool_calls,
            reasoning_content: message.reasoning_content.as_deref(),
            reasoning: message.reasoning.as_deref(),
        },
        Message::Tool {
            tool_call_id,
            content,
        } => WireMessage::Tool {
            tool_call_id,
            content,
        },
    }
}
