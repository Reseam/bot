use anyhow::Result;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use wiremock::{Mock, MockServer, ResponseTemplate, matchers::method};

use super::*;
use crate::agent::CompactionSettings;
use crate::llm::{AssistantMessage, FunctionCall, ToolCall, ToolType};
use crate::test_support::{llm_config, sse, user};

fn settings(threshold: u64, keep_recent_tokens: u64) -> CompactionSettings {
    CompactionSettings {
        context_window: threshold,
        max_output_tokens: 0,
        compact_at_tokens: u64::MAX,
        reserve_tokens: 0,
        keep_recent_tokens,
    }
}

async fn summarizer() -> Result<(MockServer, Llm)> {
    let server = MockServer::start().await;
    let body = sse(&[json!({
        "choices": [{"delta": {"content": "## Goal\nKeep helping"}, "finish_reason": "stop"}]
    })]);
    Mock::given(method("POST"))
        .respond_with(ResponseTemplate::new(200).set_body_raw(body, "text/event-stream"))
        .mount(&server)
        .await;
    let llm = Llm::new(llm_config(server.uri()))?;
    Ok((server, llm))
}

#[tokio::test]
async fn compaction_triggers_only_above_threshold() -> Result<()> {
    let (server, llm) = summarizer().await?;
    let cancel = CancellationToken::new();
    let mut below = vec![user(&"a".repeat(40)), user(&"b".repeat(40))];
    assert!(!compact_if_needed(&llm, &cancel, &mut below, None, &settings(20, 5)).await?);
    assert!(
        server
            .received_requests()
            .await
            .expect("request recording is enabled")
            .is_empty()
    );

    let mut above = vec![user(&"a".repeat(44)), user(&"b".repeat(40))];
    assert!(compact_if_needed(&llm, &cancel, &mut above, None, &settings(20, 5)).await?);
    assert_eq!(
        server
            .received_requests()
            .await
            .expect("request recording is enabled")
            .len(),
        1
    );
    Ok(())
}

#[test]
fn cut_never_starts_at_a_tool_result() {
    let transcript = vec![
        user("old"),
        Message::Assistant(AssistantMessage {
            content: None,
            tool_calls: vec![ToolCall {
                id: "call".to_owned(),
                kind: ToolType::Function,
                function: FunctionCall {
                    name: "read".to_owned(),
                    arguments: "{}".to_owned(),
                },
                extra_content: None,
            }],
            reasoning_content: None,
            reasoning: None,
        }),
        Message::Tool {
            tool_call_id: "call".to_owned(),
            content: "tool output large enough".to_owned(),
        },
        user("new"),
    ];
    let cut = cut_point(&transcript, 2).expect("older messages can be compacted");
    assert_eq!(cut, 3);
    assert!(!matches!(transcript[cut], Message::Tool { .. }));
}

#[tokio::test]
async fn summary_replaces_older_messages_and_keeps_recent_verbatim() -> Result<()> {
    let (_server, llm) = summarizer().await?;
    let recent = user("recent message");
    let mut transcript = vec![user(&"old".repeat(100)), recent.clone()];

    assert!(
        compact_if_needed(
            &llm,
            &CancellationToken::new(),
            &mut transcript,
            None,
            &settings(1, 2),
        )
        .await?
    );

    assert_eq!(transcript.len(), 2);
    assert!(matches!(
        &transcript[0],
        Message::User { content: UserContent::Text(text) }
            if text == "<conversation-summary>\n## Goal\nKeep helping\n</conversation-summary>"
    ));
    assert_eq!(transcript[1], recent);
    Ok(())
}

#[tokio::test]
async fn previous_summary_uses_update_prompt() -> Result<()> {
    let (server, llm) = summarizer().await?;
    let mut transcript = vec![
        user("<conversation-summary>\n## Goal\nOriginal\n</conversation-summary>"),
        user(&"new progress ".repeat(30)),
        user("recent"),
    ];
    compact_if_needed(
        &llm,
        &CancellationToken::new(),
        &mut transcript,
        None,
        &settings(1, 1),
    )
    .await?;

    let requests = server
        .received_requests()
        .await
        .expect("request recording is enabled");
    let body: Value = serde_json::from_slice(&requests[0].body)?;
    let prompt = body["messages"][1]["content"]
        .as_str()
        .expect("summarization request has user text");
    assert!(prompt.contains("<previous-summary>\n## Goal\nOriginal\n</previous-summary>"));
    assert!(prompt.contains("Preserve everything from the previous summary"));
    Ok(())
}

#[tokio::test]
async fn usage_estimate_counts_only_messages_appended_after_completion() -> Result<()> {
    let (server, llm) = summarizer().await?;
    let mut transcript = vec![user(&"old".repeat(1_000)), user("recent")];
    let usage = ContextUsage {
        tokens: 10,
        transcript_len: transcript.len(),
    };
    assert!(
        !compact_if_needed(
            &llm,
            &CancellationToken::new(),
            &mut transcript,
            Some(&usage),
            &settings(20, 1),
        )
        .await?
    );
    transcript.push(user(&"appended".repeat(20)));
    assert!(
        compact_if_needed(
            &llm,
            &CancellationToken::new(),
            &mut transcript,
            Some(&usage),
            &settings(20, 1),
        )
        .await?
    );
    assert_eq!(
        server
            .received_requests()
            .await
            .expect("request recording is enabled")
            .len(),
        1
    );
    Ok(())
}
