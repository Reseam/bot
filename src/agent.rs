use anyhow::Result;
use futures::future::join_all;
use serde_json::Value;
use tokio::sync::mpsc;
use tokio_util::sync::CancellationToken;

use crate::llm::{
    Completion, ContentPart, Delta, FinishReason, ImageUrl, Llm, Message, ToolCall, UserContent,
};
use crate::text::truncate_output;
use crate::tools::{ImageData, ToolOutput, ToolSet};

mod compaction;

use compaction::{ContextUsage, compact_if_needed};

pub struct CompactionSettings {
    pub context_window: u64,
    pub max_output_tokens: u64,
    pub reserve_tokens: u64,
    pub keep_recent_tokens: u64,
}

pub enum AgentEvent {
    TurnStarted,
    Text(String),
    ToolStarted {
        id: String,
        name: String,
    },
    ToolFinished {
        id: String,
        name: String,
        is_error: bool,
    },
    Compacted,
}

#[derive(Debug, Eq, PartialEq)]
pub enum Outcome {
    Finished,
    Cancelled,
    TurnLimit,
}

pub struct Agent<'a> {
    pub llm: &'a Llm,
    pub tools: &'a ToolSet,
    pub system: &'a str,
    pub max_turns: u32,
    pub compaction: CompactionSettings,
}

impl Agent<'_> {
    pub async fn run(
        &self,
        cancel: &CancellationToken,
        transcript: &mut Vec<Message>,
        steering: &mut mpsc::UnboundedReceiver<Message>,
        events: &mpsc::UnboundedSender<AgentEvent>,
    ) -> Result<Outcome> {
        let specs = self.tools.specs();
        let mut latest_usage = None;
        for _ in 0..self.max_turns {
            drain_steering(steering, transcript);
            if cancel.is_cancelled() {
                return Ok(Outcome::Cancelled);
            }

            let compacted = compact_if_needed(
                self.llm,
                cancel,
                transcript,
                latest_usage.as_ref(),
                &self.compaction,
            )
            .await;
            if compacted.is_err() && cancel.is_cancelled() {
                return Ok(Outcome::Cancelled);
            }
            if compacted? {
                let _ = events.send(AgentEvent::Compacted);
            }

            let _ = events.send(AgentEvent::TurnStarted);
            let completion = self
                .llm
                .complete(self.system, transcript, &specs, cancel, |delta| {
                    if let Delta::Text(text) = delta {
                        let _ = events.send(AgentEvent::Text(text));
                    }
                })
                .await;
            let Completion {
                message,
                finish_reason,
                usage,
            } = match completion {
                Ok(completion) => completion,
                Err(_) if cancel.is_cancelled() => return Ok(Outcome::Cancelled),
                Err(error) => return Err(error),
            };
            let tool_calls = message.tool_calls.clone();
            transcript.push(Message::Assistant(message));
            latest_usage = usage.map(|usage| ContextUsage {
                tokens: usage.prompt_tokens + usage.completion_tokens,
                transcript_len: transcript.len(),
            });

            if tool_calls.is_empty() {
                if drain_steering(steering, transcript) == 0 {
                    return Ok(Outcome::Finished);
                }
                continue;
            }

            let results = if finish_reason == FinishReason::Length {
                error_results(
                    &tool_calls,
                    "tool call was cut off by the output token limit; re-issue it with complete arguments",
                )
            } else {
                match execute_tools(self.tools, cancel, &tool_calls, events).await {
                    Some(results) => results,
                    None => {
                        append_results(
                            transcript,
                            error_results(&tool_calls, "cancelled by the user"),
                        );
                        return Ok(Outcome::Cancelled);
                    }
                }
            };
            append_results(transcript, results);
        }
        Ok(Outcome::TurnLimit)
    }
}

struct ExecutedTool {
    call: ToolCall,
    result: Result<ToolOutput, String>,
}

fn error_results(calls: &[ToolCall], error: &str) -> Vec<ExecutedTool> {
    calls
        .iter()
        .map(|call| ExecutedTool {
            call: call.clone(),
            result: Err(error.to_owned()),
        })
        .collect()
}

async fn execute_tools(
    tools: &ToolSet,
    cancel: &CancellationToken,
    calls: &[ToolCall],
    events: &mpsc::UnboundedSender<AgentEvent>,
) -> Option<Vec<ExecutedTool>> {
    let futures = calls.iter().map(|call| async move {
        let name = call.function.name.clone();
        let _ = events.send(AgentEvent::ToolStarted {
            id: call.id.clone(),
            name: name.clone(),
        });
        let result = execute_tool(tools, call).await;
        let _ = events.send(AgentEvent::ToolFinished {
            id: call.id.clone(),
            name,
            is_error: result.is_err(),
        });
        ExecutedTool {
            call: call.clone(),
            result,
        }
    });
    tokio::select! {
        () = cancel.cancelled() => None,
        results = join_all(futures) => Some(results),
    }
}

async fn execute_tool(tools: &ToolSet, call: &ToolCall) -> Result<ToolOutput, String> {
    let Some(tool) = tools.get(&call.function.name) else {
        return Err(format!("unknown tool `{}`", call.function.name));
    };
    let arguments: Value = serde_json::from_str(&call.function.arguments)
        .map_err(|error| format!("invalid JSON arguments: {error}"))?;
    tool.execute(arguments)
        .await
        .map_err(|error| format!("{error:#}"))
}

fn append_results(transcript: &mut Vec<Message>, results: Vec<ExecutedTool>) {
    let mut images = Vec::new();
    let mut image_sources = Vec::new();
    for result in results {
        let content = match result.result {
            Ok(output) => {
                if !output.images.is_empty() {
                    image_sources.push(format!(
                        "{} ({})",
                        result.call.function.name, result.call.id
                    ));
                }
                images.extend(output.images);
                truncate_output(&output.text)
            }
            Err(error) => truncate_output(&format!("Error: {error}")),
        };
        transcript.push(Message::Tool {
            tool_call_id: result.call.id,
            content,
        });
    }
    if !images.is_empty() {
        transcript.push(image_message(images, image_sources));
    }
}

fn image_message(images: Vec<ImageData>, sources: Vec<String>) -> Message {
    let mut content = Vec::with_capacity(images.len() + 1);
    content.push(ContentPart::Text {
        text: format!("Images returned by {}:", sources.join(", ")),
    });
    content.extend(images.into_iter().map(|image| ContentPart::ImageUrl {
        image_url: ImageUrl {
            url: image.data_url(),
        },
    }));
    Message::User {
        content: UserContent::from_parts(content),
    }
}

fn drain_steering(
    steering: &mut mpsc::UnboundedReceiver<Message>,
    transcript: &mut Vec<Message>,
) -> usize {
    let mut count = 0;
    while let Ok(message) = steering.try_recv() {
        transcript.push(message);
        count += 1;
    }
    count
}

#[cfg(test)]
mod tests;
