use std::collections::BTreeMap;

use anyhow::{Context, Result, bail};
use eventsource_stream::Eventsource;
use futures::StreamExt;
use reqwest::Response;
use serde::Deserialize;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::llm::{
    AssistantMessage, Completion, Delta, FinishReason, FunctionCall, ToolCall, ToolType, Usage,
};

#[derive(Deserialize)]
struct StreamChunk {
    #[serde(default)]
    choices: Vec<Choice>,
    usage: Option<StreamUsage>,
    error: Option<ApiError>,
}

#[derive(Deserialize)]
struct StreamUsage {
    #[serde(default)]
    prompt_tokens: u64,
    #[serde(default)]
    completion_tokens: u64,
    prompt_tokens_details: Option<PromptTokensDetails>,
}

#[derive(Deserialize)]
struct PromptTokensDetails {
    #[serde(default)]
    cached_tokens: u64,
}

#[derive(Deserialize)]
struct Choice {
    #[serde(default)]
    delta: ChoiceDelta,
    finish_reason: Option<String>,
}

#[derive(Default, Deserialize)]
struct ChoiceDelta {
    content: Option<String>,
    reasoning_content: Option<String>,
    reasoning: Option<String>,
    #[serde(default)]
    tool_calls: Vec<ToolCallDelta>,
}

#[derive(Deserialize)]
struct ToolCallDelta {
    index: Option<usize>,
    id: Option<String>,
    function: Option<FunctionCallDelta>,
    extra_content: Option<Value>,
}

#[derive(Deserialize)]
struct FunctionCallDelta {
    name: Option<String>,
    arguments: Option<String>,
}

#[derive(Deserialize)]
struct ApiError {
    message: String,
}

#[derive(Default)]
struct PendingToolCall {
    id: String,
    name: String,
    arguments: String,
    extra_content: Option<Value>,
}

#[derive(Default)]
struct Accumulator {
    content: String,
    reasoning_content: String,
    reasoning: String,
    tool_calls: BTreeMap<usize, PendingToolCall>,
    last_tool_index: Option<usize>,
}

pub(in crate::llm) async fn parse_stream(
    response: Response,
    cancel: &CancellationToken,
    on_delta: &mut impl FnMut(Delta),
) -> Result<Completion> {
    let mut stream = response.bytes_stream().eventsource();
    let mut accumulator = Accumulator::default();
    let mut finish_reason = None;
    let mut usage = None;

    loop {
        let event = tokio::select! {
            () = cancel.cancelled() => bail!("LLM request cancelled"),
            event = stream.next() => event,
        };
        let Some(event) = event else { break };
        let event = event.context("failed to read LLM event stream")?;
        if event.data == "[DONE]" {
            break;
        }
        let chunk: StreamChunk =
            serde_json::from_str(&event.data).context("failed to parse LLM stream chunk")?;
        if let Some(error) = chunk.error {
            bail!("LLM stream error: {}", error.message);
        }
        usage = chunk.usage.map(Usage::from).or(usage);
        for choice in chunk.choices {
            accumulator.push(choice.delta, on_delta);
            if let Some(reason) = choice.finish_reason {
                finish_reason = Some(parse_finish_reason(reason));
            }
        }
    }

    Ok(Completion {
        message: accumulator.finish(),
        finish_reason: finish_reason.context("LLM stream ended without a finish reason")?,
        usage,
    })
}

impl Accumulator {
    fn push(&mut self, delta: ChoiceDelta, on_delta: &mut impl FnMut(Delta)) {
        if let Some(text) = delta.content {
            self.content.push_str(&text);
            on_delta(Delta::Text(text));
        }
        for (fragment, target) in [
            (delta.reasoning_content, &mut self.reasoning_content),
            (delta.reasoning, &mut self.reasoning),
        ] {
            if let Some(fragment) = fragment {
                target.push_str(&fragment);
                on_delta(Delta::Reasoning(fragment));
            }
        }
        for fragment in delta.tool_calls {
            self.push_tool_call(fragment);
        }
    }

    fn push_tool_call(&mut self, fragment: ToolCallDelta) {
        let starts_call = fragment.id.is_some()
            || fragment
                .function
                .as_ref()
                .is_some_and(|function| function.name.is_some());
        let index = fragment.index.unwrap_or_else(|| {
            if starts_call {
                self.tool_calls
                    .last_key_value()
                    .map_or(0, |(index, _)| index + 1)
            } else {
                self.last_tool_index.unwrap_or(0)
            }
        });
        self.last_tool_index = Some(index);
        let call = self.tool_calls.entry(index).or_default();
        if let Some(id) = fragment.id {
            call.id.push_str(&id);
        }
        if let Some(function) = fragment.function {
            if let Some(name) = function.name {
                call.name.push_str(&name);
            }
            if let Some(arguments) = function.arguments {
                call.arguments.push_str(&arguments);
            }
        }
        if let Some(extra_content) = fragment.extra_content {
            call.extra_content = Some(extra_content);
        }
    }

    fn finish(self) -> AssistantMessage {
        let tool_calls = self
            .tool_calls
            .into_values()
            .enumerate()
            .map(|(position, call)| ToolCall {
                id: if call.id.is_empty() {
                    format!("call_{}", position + 1)
                } else {
                    call.id
                },
                kind: ToolType::Function,
                function: FunctionCall {
                    name: call.name,
                    arguments: call.arguments,
                },
                extra_content: call.extra_content,
            })
            .collect();
        AssistantMessage {
            content: non_empty(self.content),
            tool_calls,
            reasoning_content: non_empty(self.reasoning_content),
            reasoning: non_empty(self.reasoning),
            anthropic_content: Vec::new(),
        }
    }
}

fn non_empty(text: String) -> Option<String> {
    (!text.is_empty()).then_some(text)
}

fn parse_finish_reason(reason: String) -> FinishReason {
    match reason.as_str() {
        "stop" => FinishReason::Stop,
        "length" => FinishReason::Length,
        "tool_calls" => FinishReason::ToolCalls,
        "content_filter" => FinishReason::Refusal,
        _ => FinishReason::Other(reason),
    }
}

impl From<StreamUsage> for Usage {
    fn from(usage: StreamUsage) -> Self {
        Self {
            prompt_tokens: usage.prompt_tokens,
            cached_tokens: usage
                .prompt_tokens_details
                .map_or(0, |details| details.cached_tokens),
            completion_tokens: usage.completion_tokens,
        }
    }
}
