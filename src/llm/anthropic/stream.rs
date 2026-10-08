use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use eventsource_stream::Eventsource;
use futures::StreamExt;
use reqwest::Response;
use serde::Deserialize;
use tokio_util::sync::CancellationToken;

use super::{ContentBlock, tool_input};
use crate::llm::{
    AssistantMessage, Completion, Delta, FinishReason, FunctionCall, ToolCall, ToolType, Usage,
};

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum Event {
    MessageStart {
        message: MessageStart,
    },
    ContentBlockStart {
        index: usize,
        content_block: BlockStart,
    },
    ContentBlockDelta {
        index: usize,
        delta: BlockDelta,
    },
    MessageDelta {
        delta: MessageDelta,
        usage: StreamUsage,
    },
    MessageStop,
    Error {
        error: ApiError,
    },
    #[serde(other)]
    Other,
}

#[derive(Deserialize)]
struct MessageStart {
    usage: StreamUsage,
}

#[derive(Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
enum BlockStart {
    Text {
        text: String,
    },
    Thinking {
        thinking: String,
        #[serde(default)]
        signature: String,
    },
    RedactedThinking {
        data: String,
    },
    ToolUse {
        id: String,
        name: String,
    },
}

#[derive(Deserialize)]
#[serde(tag = "type")]
enum BlockDelta {
    #[serde(rename = "text_delta")]
    Text { text: String },
    #[serde(rename = "thinking_delta")]
    Thinking { thinking: String },
    #[serde(rename = "signature_delta")]
    Signature { signature: String },
    #[serde(rename = "input_json_delta")]
    InputJson { partial_json: String },
}

#[derive(Deserialize)]
struct MessageDelta {
    stop_reason: Option<String>,
}

#[derive(Default, Deserialize)]
struct StreamUsage {
    input_tokens: Option<u64>,
    output_tokens: Option<u64>,
    cache_creation_input_tokens: Option<u64>,
    cache_read_input_tokens: Option<u64>,
}

#[derive(Deserialize)]
struct ApiError {
    #[serde(rename = "type")]
    kind: String,
    message: String,
}

enum PendingBlock {
    Complete(ContentBlock),
    ToolUse {
        id: String,
        name: String,
        input: String,
    },
}

pub(in crate::llm) async fn parse_stream(
    response: Response,
    cancel: &CancellationToken,
    on_delta: &mut impl FnMut(Delta),
) -> Result<Completion> {
    let mut stream = response.bytes_stream().eventsource();
    let mut blocks = BTreeMap::new();
    let mut usage = StreamUsage::default();
    let mut stop_reason = None;

    loop {
        let event = tokio::select! {
            () = cancel.cancelled() => bail!("LLM request cancelled"),
            event = stream.next() => event,
        };
        let Some(event) = event else {
            bail!("LLM stream ended before message_stop")
        };
        let event = event.context("failed to read LLM event stream")?;
        match serde_json::from_str(&event.data).context("failed to parse LLM stream event")? {
            Event::MessageStart { message } => usage.merge(message.usage),
            Event::ContentBlockStart {
                index,
                content_block,
            } => {
                blocks.insert(index, PendingBlock::from(content_block));
            }
            Event::ContentBlockDelta { index, delta } => {
                let block = blocks.get_mut(&index).with_context(|| {
                    format!("LLM stream sent a delta for unknown block {index}")
                })?;
                block.push(index, delta, on_delta)?;
            }
            Event::MessageDelta {
                delta,
                usage: update,
            } => {
                stop_reason = delta.stop_reason.or(stop_reason);
                usage.merge(update);
            }
            Event::MessageStop => break,
            Event::Error { error } => bail!("LLM stream error: {}: {}", error.kind, error.message),
            Event::Other => {}
        }
    }

    Ok(Completion {
        message: finish(blocks),
        finish_reason: parse_finish_reason(
            stop_reason.context("LLM stream ended without a stop reason")?,
        ),
        usage: Some(usage.total()),
    })
}

impl From<BlockStart> for PendingBlock {
    fn from(start: BlockStart) -> Self {
        match start {
            BlockStart::Text { text } => Self::Complete(ContentBlock::Text { text }),
            BlockStart::Thinking {
                thinking,
                signature,
            } => Self::Complete(ContentBlock::Thinking {
                thinking,
                signature,
            }),
            BlockStart::RedactedThinking { data } => {
                Self::Complete(ContentBlock::RedactedThinking { data })
            }
            BlockStart::ToolUse { id, name } => Self::ToolUse {
                id,
                name,
                input: String::new(),
            },
        }
    }
}

impl PendingBlock {
    fn push(
        &mut self,
        index: usize,
        delta: BlockDelta,
        on_delta: &mut impl FnMut(Delta),
    ) -> Result<()> {
        match (self, delta) {
            (Self::Complete(ContentBlock::Text { text }), BlockDelta::Text { text: fragment }) => {
                text.push_str(&fragment);
                on_delta(Delta::Text(fragment));
            }
            (
                Self::Complete(ContentBlock::Thinking { thinking, .. }),
                BlockDelta::Thinking { thinking: fragment },
            ) => {
                thinking.push_str(&fragment);
                on_delta(Delta::Reasoning(fragment));
            }
            (
                Self::Complete(ContentBlock::Thinking { signature, .. }),
                BlockDelta::Signature {
                    signature: fragment,
                },
            ) => signature.push_str(&fragment),
            (Self::ToolUse { input, .. }, BlockDelta::InputJson { partial_json }) => {
                input.push_str(&partial_json);
            }
            _ => bail!("LLM stream sent a delta that does not match block {index}"),
        }
        Ok(())
    }
}

impl StreamUsage {
    fn merge(&mut self, update: Self) {
        self.input_tokens = update.input_tokens.or(self.input_tokens);
        self.output_tokens = update.output_tokens.or(self.output_tokens);
        self.cache_creation_input_tokens = update
            .cache_creation_input_tokens
            .or(self.cache_creation_input_tokens);
        self.cache_read_input_tokens = update
            .cache_read_input_tokens
            .or(self.cache_read_input_tokens);
    }

    fn total(&self) -> Usage {
        let cached_tokens = self.cache_read_input_tokens.unwrap_or_default();
        Usage {
            prompt_tokens: self.input_tokens.unwrap_or_default()
                + self.cache_creation_input_tokens.unwrap_or_default()
                + cached_tokens,
            cached_tokens,
            completion_tokens: self.output_tokens.unwrap_or_default(),
        }
    }
}

fn finish(blocks: BTreeMap<usize, PendingBlock>) -> AssistantMessage {
    let mut text = String::new();
    let mut tool_calls = Vec::new();
    let mut content = Vec::with_capacity(blocks.len());
    for block in blocks.into_values() {
        match block {
            PendingBlock::Complete(block) => {
                if let ContentBlock::Text { text: fragment } = &block {
                    text.push_str(fragment);
                }
                content.push(block);
            }
            PendingBlock::ToolUse { id, name, input } => {
                let arguments = if input.is_empty() {
                    "{}".to_owned()
                } else {
                    input
                };
                content.push(ContentBlock::ToolUse {
                    id: id.clone(),
                    name: name.clone(),
                    input: tool_input(&arguments),
                });
                tool_calls.push(ToolCall {
                    id,
                    kind: ToolType::Function,
                    function: FunctionCall { name, arguments },
                    extra_content: None,
                });
            }
        }
    }
    AssistantMessage {
        content: (!text.is_empty()).then_some(text),
        tool_calls,
        reasoning_content: None,
        reasoning: None,
        anthropic_content: content,
    }
}

fn parse_finish_reason(reason: String) -> FinishReason {
    match reason.as_str() {
        "end_turn" | "stop_sequence" => FinishReason::Stop,
        "max_tokens" | "model_context_window_exceeded" => FinishReason::Length,
        "tool_use" => FinishReason::ToolCalls,
        "refusal" => FinishReason::Refusal,
        _ => FinishReason::Other(reason),
    }
}
