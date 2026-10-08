use anyhow::Result;
use serde_json::{Value, json};
use tokio_util::sync::CancellationToken;
use wiremock::matchers::{header, method, path};
use wiremock::{Mock, MockServer, ResponseTemplate};

use super::*;
use crate::config::ProviderKind;
use crate::llm::{Delta, FinishReason, FunctionCall, ImageUrl, ToolCall, ToolType, Usage};
use crate::test_support::{anthropic_sse, test_model, user};

fn tool_call(id: &str, arguments: &str) -> ToolCall {
    ToolCall {
        id: id.to_owned(),
        kind: ToolType::Function,
        function: FunctionCall {
            name: "bash".to_owned(),
            arguments: arguments.to_owned(),
        },
        extra_content: None,
    }
}

fn tool_result(id: &str, content: &str) -> Message {
    Message::Tool {
        tool_call_id: id.to_owned(),
        content: content.to_owned(),
    }
}

fn wire(transcript: &[Message]) -> Result<Value> {
    Ok(serde_json::to_value(wire_messages(transcript))?)
}

#[test]
fn tool_results_images_and_steering_share_one_user_turn() -> Result<()> {
    let transcript = vec![
        user("start"),
        Message::Assistant(AssistantMessage {
            content: Some("Checking.".to_owned()),
            tool_calls: vec![tool_call("a", "{}"), tool_call("b", "{}")],
            ..AssistantMessage::default()
        }),
        tool_result("a", "first"),
        tool_result("b", "second"),
        Message::User {
            content: UserContent::Parts(vec![
                ContentPart::Text {
                    text: "Images returned by bash (b):".to_owned(),
                },
                ContentPart::ImageUrl {
                    image_url: ImageUrl {
                        url: "data:image/png;base64,AA==".to_owned(),
                    },
                },
            ]),
        },
        user("new direction"),
    ];

    assert_eq!(
        wire(&transcript)?,
        json!([
            {"role":"user","content":[{"type":"text","text":"start"}]},
            {"role":"assistant","content":[
                {"type":"text","text":"Checking."},
                {"type":"tool_use","id":"a","name":"bash","input":{}},
                {"type":"tool_use","id":"b","name":"bash","input":{}}
            ]},
            {"role":"user","content":[
                {"type":"tool_result","tool_use_id":"a","content":"first"},
                {"type":"tool_result","tool_use_id":"b","content":"second"},
                {"type":"text","text":"Images returned by bash (b):"},
                {"type":"image","source":{"type":"base64","media_type":"image/png","data":"AA=="}},
                {"type":"text","text":"new direction"}
            ]}
        ])
    );
    Ok(())
}

#[test]
fn anthropic_content_is_replayed_verbatim_and_in_order() -> Result<()> {
    let transcript = vec![
        user("start"),
        Message::Assistant(AssistantMessage {
            content: Some("Looking.".to_owned()),
            tool_calls: vec![tool_call("toolu_1", r#"{"command":"ls"}"#)],
            anthropic_content: vec![
                ContentBlock::Thinking {
                    thinking: String::new(),
                    signature: "first".to_owned(),
                },
                ContentBlock::Text {
                    text: "Looking.".to_owned(),
                },
                ContentBlock::RedactedThinking {
                    data: "opaque".to_owned(),
                },
                ContentBlock::ToolUse {
                    id: "toolu_1".to_owned(),
                    name: "bash".to_owned(),
                    input: json!({"command":"ls"}),
                },
            ],
            ..AssistantMessage::default()
        }),
        tool_result("toolu_1", "file"),
    ];

    assert_eq!(
        wire(&transcript)?[1]["content"],
        json!([
            {"type":"thinking","thinking":"","signature":"first"},
            {"type":"text","text":"Looking."},
            {"type":"redacted_thinking","data":"opaque"},
            {"type":"tool_use","id":"toolu_1","name":"bash","input":{"command":"ls"}}
        ])
    );
    Ok(())
}

#[test]
fn foreign_tool_calls_are_normalized() -> Result<()> {
    let transcript = vec![
        user("start"),
        Message::Assistant(AssistantMessage {
            tool_calls: vec![tool_call("functions.bash:0", "{\"command\":")],
            ..AssistantMessage::default()
        }),
        tool_result("functions.bash:0", "Error: invalid JSON arguments"),
    ];

    let messages = wire(&transcript)?;
    assert_eq!(
        messages[1]["content"],
        json!([{"type":"tool_use","id":"functions_bash_0","name":"bash","input":{}}])
    );
    assert_eq!(
        messages[2]["content"][0]["tool_use_id"],
        json!("functions_bash_0")
    );
    Ok(())
}

#[test]
fn empty_turns_are_dropped_so_roles_alternate() -> Result<()> {
    let transcript = vec![
        user("first"),
        Message::Assistant(AssistantMessage::default()),
        user("  "),
        user("second"),
    ];

    assert_eq!(
        wire(&transcript)?,
        json!([{"role":"user","content":[
            {"type":"text","text":"first"},
            {"type":"text","text":"second"}
        ]}])
    );
    Ok(())
}

#[tokio::test]
async fn streams_thinking_text_and_tool_use() -> Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .and(path("/v1/messages"))
        .and(header("x-api-key", "test-key"))
        .and(header("anthropic-version", API_VERSION))
        .respond_with(ResponseTemplate::new(200).set_body_raw(
            anthropic_sse(&[
                json!({"type":"message_start","message":{"usage":{"input_tokens":10,"cache_creation_input_tokens":5,"cache_read_input_tokens":100,"output_tokens":1}}}),
                json!({"type":"content_block_start","index":0,"content_block":{"type":"thinking","thinking":"","signature":""}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"thinking_delta","thinking":"plan"}}),
                json!({"type":"content_block_delta","index":0,"delta":{"type":"signature_delta","signature":"sig"}}),
                json!({"type":"content_block_stop","index":0}),
                json!({"type":"ping"}),
                json!({"type":"content_block_start","index":1,"content_block":{"type":"text","text":""}}),
                json!({"type":"content_block_delta","index":1,"delta":{"type":"text_delta","text":"Checking."}}),
                json!({"type":"content_block_stop","index":1}),
                json!({"type":"content_block_start","index":2,"content_block":{"type":"tool_use","id":"toolu_1","name":"bash","input":{}}}),
                json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"{\"command\":"}}),
                json!({"type":"content_block_delta","index":2,"delta":{"type":"input_json_delta","partial_json":"\"ls\"}"}}),
                json!({"type":"content_block_stop","index":2}),
                json!({"type":"message_delta","delta":{"stop_reason":"tool_use"},"usage":{"output_tokens":20}}),
                json!({"type":"message_stop"}),
            ]),
            "text/event-stream",
        ))
        .mount(&server)
        .await;
    let model = test_model(ProviderKind::Anthropic, server.uri());
    let mut deltas = Vec::new();

    let completion = model
        .complete(
            "system",
            &[user("list files")],
            &[],
            ToolChoice::Auto,
            &CancellationToken::new(),
            |delta| deltas.push(delta),
        )
        .await?;

    assert_eq!(completion.finish_reason, FinishReason::ToolCalls);
    assert_eq!(
        deltas,
        vec![
            Delta::Reasoning("plan".to_owned()),
            Delta::Text("Checking.".to_owned())
        ]
    );
    assert_eq!(completion.message.content.as_deref(), Some("Checking."));
    assert_eq!(
        completion.message.tool_calls,
        vec![tool_call("toolu_1", r#"{"command":"ls"}"#)]
    );
    assert_eq!(
        completion.message.anthropic_content,
        vec![
            ContentBlock::Thinking {
                thinking: "plan".to_owned(),
                signature: "sig".to_owned(),
            },
            ContentBlock::Text {
                text: "Checking.".to_owned(),
            },
            ContentBlock::ToolUse {
                id: "toolu_1".to_owned(),
                name: "bash".to_owned(),
                input: json!({"command":"ls"}),
            },
        ]
    );
    assert_eq!(
        completion.usage,
        Some(Usage {
            prompt_tokens: 115,
            cached_tokens: 100,
            completion_tokens: 20,
        })
    );
    let requests = server.received_requests().await.unwrap_or_default();
    let body: Value = serde_json::from_slice(&requests[0].body)?;
    assert_eq!(
        body["system"],
        json!([{"type":"text","text":"system","cache_control":{"type":"ephemeral","ttl":"1h"}}])
    );
    assert_eq!(body["stream"], json!(true));
    assert_eq!(
        body["cache_control"],
        json!({"type":"ephemeral","ttl":"1h"})
    );
    assert!(body.get("tools").is_none());
    assert!(body.get("tool_choice").is_none());
    Ok(())
}
