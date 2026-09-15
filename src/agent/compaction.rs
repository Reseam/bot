use anyhow::{Context, Result, anyhow};
use tokio_util::sync::CancellationToken;

use super::CompactionSettings;
use crate::llm::{ContentPart, Llm, Message, UserContent};

pub(super) const SUMMARIZATION_SYSTEM_PROMPT: &str = "You are a context summarization assistant. Read a conversation between Discord users and an AI assistant and produce a structured summary in the exact format requested. Do not continue the conversation. Do not answer questions in it. Output only the summary.";
const SUMMARY_FORMAT: &str = "## Goal\n## Constraints & Preferences\n## Progress\n### Done\n### In Progress\n### Blocked\n## Key Decisions\n## Next Steps\n## Critical Context";
const NEW_SUMMARY_INSTRUCTIONS: &str =
    "Summarize the conversation above using exactly the requested sections.";
const UPDATE_SUMMARY_INSTRUCTIONS: &str = "Update the previous summary with the new conversation above. Preserve everything from the previous summary, add new progress and decisions, move finished items to Done, and update Next Steps.";
const CRITICAL_CONTEXT_INSTRUCTIONS: &str = "Critical Context must preserve exact Discord user, channel, and message ids, links, repository names, issue numbers, file paths, commands, and error messages.";
const SUMMARY_PREFIX: &str = "<conversation-summary>\n";
const SUMMARY_SUFFIX: &str = "\n</conversation-summary>";
const TOOL_RESULT_LIMIT: usize = 2_000;
const IMAGE_CHARS: u64 = 4_800;

pub(super) struct ContextUsage {
    pub tokens: u64,
    pub transcript_len: usize,
}

pub(super) async fn compact_if_needed(
    llm: &Llm,
    cancel: &CancellationToken,
    transcript: &mut Vec<Message>,
    usage: Option<&ContextUsage>,
    settings: &CompactionSettings,
) -> Result<bool> {
    let threshold = settings
        .context_window
        .saturating_sub(settings.max_output_tokens)
        .saturating_sub(settings.reserve_tokens);
    if context_estimate(transcript, usage) <= threshold {
        return Ok(false);
    }
    let Some(cut) = cut_point(transcript, settings.keep_recent_tokens) else {
        return Ok(false);
    };
    let older = &transcript[..cut];
    let (previous, serialized_start) =
        previous_summary(older).map_or((None, 0), |summary| (Some(summary), 1));
    let serialized = serialize_messages(&older[serialized_start..]);
    let instructions = match previous {
        Some(_) => UPDATE_SUMMARY_INSTRUCTIONS,
        None => NEW_SUMMARY_INSTRUCTIONS,
    };
    let instructions = format!(
        "{instructions}\n\nUse exactly these sections:\n{SUMMARY_FORMAT}\n\n{CRITICAL_CONTEXT_INSTRUCTIONS}"
    );
    let prompt = match previous {
        Some(previous) => format!(
            "<previous-summary>\n{previous}\n</previous-summary>\n\n{serialized}\n\n{instructions}"
        ),
        None => format!("{serialized}\n\n{instructions}"),
    };
    let completion = llm
        .complete(
            SUMMARIZATION_SYSTEM_PROMPT,
            &[Message::User {
                content: UserContent::Text(prompt),
            }],
            &[],
            cancel,
            |_| {},
        )
        .await
        .context("failed to compact conversation")?;
    let summary = completion
        .message
        .content
        .filter(|text| !text.trim().is_empty())
        .ok_or_else(|| anyhow!("failed to compact conversation: summarizer returned no text"))?;
    transcript.splice(
        ..cut,
        [Message::User {
            content: UserContent::Text(format!("{SUMMARY_PREFIX}{summary}{SUMMARY_SUFFIX}")),
        }],
    );
    Ok(true)
}

pub(super) fn context_estimate(transcript: &[Message], usage: Option<&ContextUsage>) -> u64 {
    usage.map_or_else(
        || estimate_messages(transcript),
        |usage| {
            usage.tokens
                + transcript
                    .get(usage.transcript_len..)
                    .map_or(0, estimate_messages)
        },
    )
}

fn cut_point(transcript: &[Message], keep_recent_tokens: u64) -> Option<usize> {
    let mut recent_tokens = 0;
    let mut cut = transcript.len();
    for (index, message) in transcript.iter().enumerate().rev() {
        recent_tokens += estimate_message(message);
        cut = index;
        if recent_tokens >= keep_recent_tokens {
            break;
        }
    }
    while matches!(transcript.get(cut), Some(Message::Tool { .. })) {
        cut += 1;
    }
    (cut > 0 && cut < transcript.len()).then_some(cut)
}

fn estimate_messages(messages: &[Message]) -> u64 {
    messages.iter().map(estimate_message).sum()
}

fn estimate_message(message: &Message) -> u64 {
    let chars = match message {
        Message::System { content } => char_count(content),
        Message::User { content } => match content {
            UserContent::Text(text) => char_count(text),
            UserContent::Parts(parts) => parts
                .iter()
                .map(|part| match part {
                    ContentPart::Text { text } => char_count(text),
                    ContentPart::ImageUrl { .. } => IMAGE_CHARS,
                })
                .sum(),
        },
        Message::Assistant(message) => {
            message
                .content
                .iter()
                .chain(message.reasoning_content.iter())
                .chain(message.reasoning.iter())
                .map(|text| char_count(text))
                .sum::<u64>()
                + message
                    .tool_calls
                    .iter()
                    .map(|call| {
                        char_count(&call.function.name) + char_count(&call.function.arguments)
                    })
                    .sum::<u64>()
        }
        Message::Tool { content, .. } => char_count(content),
    };
    chars.div_ceil(4)
}

fn char_count(text: &str) -> u64 {
    text.chars().count() as u64
}

fn previous_summary(messages: &[Message]) -> Option<&str> {
    let Message::User {
        content: UserContent::Text(text),
    } = messages.first()?
    else {
        return None;
    };
    text.strip_prefix(SUMMARY_PREFIX)
        .and_then(|text| text.strip_suffix(SUMMARY_SUFFIX))
}

fn serialize_messages(messages: &[Message]) -> String {
    messages
        .iter()
        .map(|message| match message {
            Message::System { content } => format!("[System]: {content}"),
            Message::User { content } => format!("[User]: {}", serialize_user(content)),
            Message::Assistant(message) => {
                let mut lines = message
                    .content
                    .as_ref()
                    .map(|content| vec![format!("[Assistant]: {content}")])
                    .unwrap_or_default();
                if !message.tool_calls.is_empty() {
                    lines.push(format!(
                        "[Assistant tool calls]: {}",
                        message
                            .tool_calls
                            .iter()
                            .map(|call| format!(
                                "{}({})",
                                call.function.name, call.function.arguments
                            ))
                            .collect::<Vec<_>>()
                            .join("; ")
                    ));
                }
                lines.join("\n")
            }
            Message::Tool { content, .. } => format!(
                "[Tool result]: {}",
                content.chars().take(TOOL_RESULT_LIMIT).collect::<String>()
            ),
        })
        .collect::<Vec<_>>()
        .join("\n\n")
}

fn serialize_user(content: &UserContent) -> String {
    match content {
        UserContent::Text(text) => text.clone(),
        UserContent::Parts(parts) => parts
            .iter()
            .map(|part| match part {
                ContentPart::Text { text } => text.clone(),
                ContentPart::ImageUrl { .. } => "[image]".to_owned(),
            })
            .collect::<Vec<_>>()
            .join("\n"),
    }
}

#[cfg(test)]
mod tests;
