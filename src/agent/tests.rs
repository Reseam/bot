use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;
use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::{Value, json};
use tokio::sync::{Barrier, mpsc};
use tokio_util::sync::CancellationToken;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate, matchers::method};

use super::*;
use crate::test_support::{llm_config, sse, user};
use crate::tools::Tool;

#[derive(Clone)]
struct SequenceResponder {
    next: Arc<AtomicUsize>,
    bodies: Arc<Vec<String>>,
}

impl Respond for SequenceResponder {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        let index = self.next.fetch_add(1, Ordering::SeqCst);
        let body = self
            .bodies
            .get(index)
            .or_else(|| self.bodies.last())
            .expect("test responder always has at least one body");
        ResponseTemplate::new(200).set_body_raw(body.clone(), "text/event-stream")
    }
}

fn assistant_text(text: &str) -> String {
    sse(&[json!({
        "choices":[{"delta":{"content":text},"finish_reason":"stop"}]
    })])
}

fn tool_turn(calls: Value, finish_reason: &str) -> String {
    sse(&[json!({
        "choices":[{"delta":{"tool_calls":calls},"finish_reason":finish_reason}]
    })])
}

fn tool_call(index: usize, id: &str, name: &str, arguments: &str) -> Value {
    json!({
        "index":index,
        "id":id,
        "type":"function",
        "function":{"name":name,"arguments":arguments}
    })
}

async fn llm_with_responses(responses: Vec<String>) -> Result<(MockServer, Llm)> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(SequenceResponder {
            next: Arc::new(AtomicUsize::new(0)),
            bodies: Arc::new(responses),
        })
        .mount(&server)
        .await;
    let llm = Llm::new(llm_config(server.uri()))?;
    Ok((server, llm))
}

#[derive(Deserialize, JsonSchema)]
struct EchoArgs {
    text: String,
}

#[derive(Deserialize, JsonSchema)]
struct NoArgs {}

fn text(value: impl Into<String>) -> ToolOutput {
    ToolOutput {
        text: value.into(),
        images: Vec::new(),
    }
}

fn echo_tool() -> Tool {
    Tool::new(
        "echo",
        "Echo text",
        (),
        |_state, arguments: EchoArgs| async move { Ok(text(arguments.text)) },
    )
}

async fn run_agent(
    llm: &Llm,
    tools: &ToolSet,
    max_turns: u32,
    transcript: &mut Vec<Message>,
    steering: &mut mpsc::UnboundedReceiver<Message>,
) -> Result<Outcome> {
    let (events, _event_receiver) = mpsc::unbounded_channel();
    Agent {
        llm,
        tools,
        system: "system",
        max_turns,
        compaction: CompactionSettings {
            context_window: u64::MAX,
            max_output_tokens: 0,
            compact_at_tokens: u64::MAX,
            reserve_tokens: 0,
            keep_recent_tokens: 20_000,
        },
    }
    .run(&CancellationToken::new(), transcript, steering, &events)
    .await
}

#[tokio::test]
async fn tool_call_then_final_text_has_ordered_transcript() -> Result<()> {
    let (_server, llm) = llm_with_responses(vec![
        tool_turn(
            json!([tool_call(0, "one", "echo", r#"{"text":"result"}"#)]),
            "tool_calls",
        ),
        assistant_text("done"),
    ])
    .await?;
    let tools = ToolSet::new(vec![echo_tool()]);
    let mut transcript = vec![user("start")];
    let (_steer, mut steering) = mpsc::unbounded_channel();

    let outcome = run_agent(&llm, &tools, 4, &mut transcript, &mut steering).await?;

    assert_eq!(outcome, Outcome::Finished);
    assert!(matches!(transcript[1], Message::Assistant(_)));
    assert!(
        matches!(&transcript[2], Message::Tool { tool_call_id, content } if tool_call_id == "one" && content == "result")
    );
    assert!(
        matches!(&transcript[3], Message::Assistant(message) if message.content.as_deref() == Some("done"))
    );
    Ok(())
}

#[tokio::test]
async fn parallel_tools_finish_without_deadlock_and_keep_source_order() -> Result<()> {
    let calls = json!([
        tool_call(0, "first", "wait", r#"{"text":"a"}"#),
        tool_call(1, "second", "wait", r#"{"text":"b"}"#)
    ]);
    let (_server, llm) =
        llm_with_responses(vec![tool_turn(calls, "tool_calls"), assistant_text("done")]).await?;
    let barrier = Arc::new(Barrier::new(2));
    let tool = Tool::new(
        "wait",
        "Wait together",
        (),
        move |_state, arguments: EchoArgs| {
            let barrier = Arc::clone(&barrier);
            async move {
                barrier.wait().await;
                Ok(text(arguments.text))
            }
        },
    );
    let tools = ToolSet::new(vec![tool]);
    let mut transcript = vec![user("start")];
    let (_steer, mut steering) = mpsc::unbounded_channel();

    let outcome = tokio::time::timeout(
        std::time::Duration::from_secs(2),
        run_agent(&llm, &tools, 4, &mut transcript, &mut steering),
    )
    .await??;

    assert_eq!(outcome, Outcome::Finished);
    assert!(
        matches!(&transcript[2], Message::Tool { tool_call_id, content } if tool_call_id == "first" && content == "a")
    );
    assert!(
        matches!(&transcript[3], Message::Tool { tool_call_id, content } if tool_call_id == "second" && content == "b")
    );
    Ok(())
}

#[tokio::test]
async fn unknown_tool_and_bad_arguments_are_recoverable_results() -> Result<()> {
    let calls = json!([
        tool_call(0, "unknown", "missing", "{}"),
        tool_call(1, "bad-json", "echo", "not json"),
        tool_call(2, "bad-shape", "echo", "{}")
    ]);
    let (_server, llm) = llm_with_responses(vec![
        tool_turn(calls, "tool_calls"),
        assistant_text("fixed"),
    ])
    .await?;
    let tools = ToolSet::new(vec![echo_tool()]);
    let mut transcript = vec![user("start")];
    let (_steer, mut steering) = mpsc::unbounded_channel();

    run_agent(&llm, &tools, 4, &mut transcript, &mut steering).await?;

    assert!(
        matches!(&transcript[2], Message::Tool { content, .. } if content.contains("Error: unknown tool"))
    );
    assert!(
        matches!(&transcript[3], Message::Tool { content, .. } if content.contains("Error: invalid JSON arguments"))
    );
    assert!(
        matches!(&transcript[4], Message::Tool { content, .. } if content.contains("missing field `text`"))
    );
    Ok(())
}

#[tokio::test]
async fn length_finish_does_not_execute_tools() -> Result<()> {
    let (_server, llm) = llm_with_responses(vec![
        tool_turn(json!([tool_call(0, "cut", "count", "{")]), "length"),
        assistant_text("retried"),
    ])
    .await?;
    let calls = Arc::new(AtomicUsize::new(0));
    let tool = Tool::new("count", "Count calls", (), {
        let calls = Arc::clone(&calls);
        move |_state, _arguments: NoArgs| {
            calls.fetch_add(1, Ordering::SeqCst);
            async { Ok(text("called")) }
        }
    });
    let tools = ToolSet::new(vec![tool]);
    let mut transcript = vec![user("start")];
    let (_steer, mut steering) = mpsc::unbounded_channel();

    run_agent(&llm, &tools, 4, &mut transcript, &mut steering).await?;

    assert_eq!(calls.load(Ordering::SeqCst), 0);
    assert!(
        matches!(&transcript[2], Message::Tool { content, .. } if content.contains("output token limit"))
    );
    Ok(())
}

#[tokio::test]
async fn cancellation_adds_results_for_every_running_tool() -> Result<()> {
    let calls = json!([
        tool_call(0, "first", "wait", "{}"),
        tool_call(1, "second", "wait", "{}")
    ]);
    let (_server, llm) = llm_with_responses(vec![tool_turn(calls, "tool_calls")]).await?;
    let tool = Tool::new("wait", "Wait", (), |_state, _arguments: NoArgs| {
        std::future::pending()
    });
    let tools = ToolSet::new(vec![tool]);
    let mut transcript = vec![user("start")];
    let (_steer, mut steering) = mpsc::unbounded_channel();
    let (events, mut event_receiver) = mpsc::unbounded_channel();
    let cancel = CancellationToken::new();
    let agent = Agent {
        llm: &llm,
        tools: &tools,
        system: "system",
        max_turns: 2,
        compaction: CompactionSettings {
            context_window: u64::MAX,
            max_output_tokens: 0,
            compact_at_tokens: u64::MAX,
            reserve_tokens: 0,
            keep_recent_tokens: 20_000,
        },
    };
    let outcome = {
        let run = agent.run(&cancel, &mut transcript, &mut steering, &events);
        tokio::pin!(run);

        loop {
            tokio::select! {
                event = event_receiver.recv() => {
                    if matches!(event, Some(AgentEvent::ToolStarted { .. })) {
                        break;
                    }
                }
                outcome = &mut run => panic!("run ended before a tool started: {outcome:?}"),
            }
        }
        cancel.cancel();
        run.await?
    };

    assert_eq!(outcome, Outcome::Cancelled);
    assert_eq!(transcript.len(), 4);
    assert!(matches!(transcript[1], Message::Assistant(_)));
    assert!(
        matches!(&transcript[2], Message::Tool { content, .. } if content == "Error: cancelled by the user")
    );
    assert!(
        matches!(&transcript[3], Message::Tool { content, .. } if content == "Error: cancelled by the user")
    );
    Ok(())
}

#[tokio::test]
async fn steering_is_inserted_before_the_next_turn() -> Result<()> {
    let (_server, llm) = llm_with_responses(vec![
        tool_turn(json!([tool_call(0, "steer", "queue", "{}")]), "tool_calls"),
        assistant_text("steered"),
    ])
    .await?;
    let (steer, mut steering) = mpsc::unbounded_channel();
    let tool = Tool::new(
        "queue",
        "Queue steering",
        (),
        move |_state, _arguments: NoArgs| {
            let steer = steer.clone();
            async move {
                let _ = steer.send(user("new direction"));
                Ok(text("queued"))
            }
        },
    );
    let tools = ToolSet::new(vec![tool]);
    let mut transcript = vec![user("start")];

    run_agent(&llm, &tools, 4, &mut transcript, &mut steering).await?;

    assert!(matches!(
        &transcript[3],
        Message::User { content: UserContent::Text(text) } if text == "new direction"
    ));
    assert!(matches!(transcript[4], Message::Assistant(_)));
    Ok(())
}
