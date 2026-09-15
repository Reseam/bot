use std::sync::Arc;
use std::sync::atomic::{AtomicUsize, Ordering};

use anyhow::Result;
use serde_json::json;
use tokio_util::sync::CancellationToken;
use wiremock::{Mock, MockServer, Request, Respond, ResponseTemplate, matchers::method};

use super::*;
use crate::test_support::{llm_config, sse, user};

fn stream_response(events: &[Value]) -> ResponseTemplate {
    ResponseTemplate::new(200).set_body_raw(sse(events), "text/event-stream")
}

#[tokio::test]
async fn streams_text_and_usage() -> Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(stream_response(&[
            json!({"choices":[{"delta":{"content":"hello "},"finish_reason":null}]}),
            json!({"choices":[{"delta":{"content":"world"},"finish_reason":"stop"}]}),
            json!({"choices":[],"usage":{"prompt_tokens":3,"completion_tokens":2,"total_tokens":5}}),
        ]))
        .mount(&server)
        .await;
    let llm = Llm::new(llm_config(server.uri()))?;
    let mut deltas = Vec::new();

    let completion = llm
        .complete(
            "system",
            &[user("hi")],
            &[],
            &CancellationToken::new(),
            |delta| deltas.push(delta),
        )
        .await?;

    assert_eq!(completion.finish_reason, FinishReason::Stop);
    assert_eq!(
        completion.usage.as_ref().map(|usage| usage.total_tokens),
        Some(5)
    );
    assert_eq!(
        deltas,
        vec![
            Delta::Text("hello ".to_owned()),
            Delta::Text("world".to_owned())
        ]
    );
    assert_eq!(completion.message.content.as_deref(), Some("hello world"));
    Ok(())
}

#[tokio::test]
async fn assembles_fragmented_tool_call() -> Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(stream_response(&[
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"id":"call_1","type":"function","function":{"name":"get_","arguments":"{\"zone\":"}}]},"finish_reason":null}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"index":0,"function":{"name":"time","arguments":"\"UTC\"}"}}]},"finish_reason":"tool_calls"}]}),
        ]))
        .mount(&server)
        .await;
    let llm = Llm::new(llm_config(server.uri()))?;

    let completion = llm
        .complete(
            "system",
            &[user("time")],
            &[],
            &CancellationToken::new(),
            |_| {},
        )
        .await?;

    let tool_calls = completion.message.tool_calls;
    assert_eq!(completion.finish_reason, FinishReason::ToolCalls);
    assert_eq!(tool_calls[0].id, "call_1");
    assert_eq!(tool_calls[0].function.name, "get_time");
    assert_eq!(tool_calls[0].function.arguments, r#"{"zone":"UTC"}"#);
    Ok(())
}

#[tokio::test]
async fn captures_tool_extra_content() -> Result<()> {
    let server = MockServer::start().await;
    let extra_content = json!({"google":{"thought_signature":"opaque"}});
    Mock::given(method("POST"))
        .respond_with(stream_response(&[json!({
            "choices":[{"delta":{"tool_calls":[{
                "index":0,
                "id":"signed",
                "function":{"name":"lookup","arguments":"{}"},
                "extra_content":extra_content.clone()
            }]},"finish_reason":"tool_calls"}]
        })]))
        .mount(&server)
        .await;
    let llm = Llm::new(llm_config(server.uri()))?;

    let completion = llm
        .complete(
            "system",
            &[user("lookup")],
            &[],
            &CancellationToken::new(),
            |_| {},
        )
        .await?;

    assert_eq!(
        completion.message.tool_calls[0].extra_content,
        Some(extra_content)
    );
    Ok(())
}

#[tokio::test]
async fn tool_deltas_without_indexes_start_or_append_calls() -> Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(stream_response(&[
            json!({"choices":[{"delta":{"tool_calls":[{"id":"first","function":{"name":"one","arguments":"{\"value\":"}}]},"finish_reason":null}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"function":{"arguments":"1}"}}]},"finish_reason":null}]}),
            json!({"choices":[{"delta":{"tool_calls":[{"id":"second","function":{"name":"two","arguments":"{}"}}]},"finish_reason":"tool_calls"}]}),
        ]))
        .mount(&server)
        .await;
    let llm = Llm::new(llm_config(server.uri()))?;

    let completion = llm
        .complete(
            "system",
            &[user("run")],
            &[],
            &CancellationToken::new(),
            |_| {},
        )
        .await?;

    assert_eq!(completion.message.tool_calls.len(), 2);
    assert_eq!(
        completion.message.tool_calls[0].function.arguments,
        r#"{"value":1}"#
    );
    assert_eq!(completion.message.tool_calls[1].function.name, "two");
    Ok(())
}

#[tokio::test]
async fn assigns_an_id_to_tool_calls_without_one() -> Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(stream_response(&[json!({
            "choices":[{"delta":{"tool_calls":[{
                "index":0,"function":{"name":"lookup","arguments":"{}"}
            }]},"finish_reason":"tool_calls"}]
        })]))
        .mount(&server)
        .await;
    let llm = Llm::new(llm_config(server.uri()))?;

    let completion = llm
        .complete(
            "system",
            &[user("lookup")],
            &[],
            &CancellationToken::new(),
            |_| {},
        )
        .await?;

    assert_eq!(completion.message.tool_calls[0].id, "call_1");
    Ok(())
}

#[tokio::test]
async fn echoes_reasoning_in_the_field_received() -> Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(stream_response(&[
            json!({"choices":[{"delta":{"reasoning_content":"think "},"finish_reason":null}]}),
            json!({"choices":[{"delta":{"reasoning_content":"carefully","content":"answer"},"finish_reason":"stop"}]}),
        ]))
        .mount(&server)
        .await;
    let llm = Llm::new(llm_config(server.uri()))?;

    let completion = llm
        .complete(
            "system",
            &[user("question")],
            &[],
            &CancellationToken::new(),
            |_| {},
        )
        .await?;

    assert_eq!(
        completion.message.reasoning_content.as_deref(),
        Some("think carefully")
    );
    assert!(completion.message.reasoning.is_none());
    Ok(())
}

#[tokio::test]
async fn accepts_missing_delta_and_partial_usage() -> Result<()> {
    let server = MockServer::start().await;
    Mock::given(method("POST"))
        .respond_with(stream_response(&[
            json!({"choices":[{"finish_reason":"stop"}]}),
            json!({"choices":[],"usage":{"prompt_tokens":4}}),
        ]))
        .mount(&server)
        .await;
    let llm = Llm::new(llm_config(server.uri()))?;

    let completion = llm
        .complete(
            "system",
            &[user("hi")],
            &[],
            &CancellationToken::new(),
            |_| {},
        )
        .await?;

    assert_eq!(
        completion.usage,
        Some(Usage {
            prompt_tokens: 4,
            completion_tokens: 0,
            total_tokens: 0,
        })
    );
    Ok(())
}

struct FailOnce {
    requests: Arc<AtomicUsize>,
    failure: ResponseTemplate,
}

impl Respond for FailOnce {
    fn respond(&self, _request: &Request) -> ResponseTemplate {
        if self.requests.fetch_add(1, Ordering::SeqCst) == 0 {
            self.failure.clone()
        } else {
            stream_response(&[json!({
                "choices":[{"delta":{"content":"recovered"},"finish_reason":"stop"}]
            })])
        }
    }
}

async fn complete_after_failure(
    failure: ResponseTemplate,
) -> Result<(usize, Vec<Delta>, Completion)> {
    let server = MockServer::start().await;
    let requests = Arc::new(AtomicUsize::new(0));
    Mock::given(method("POST"))
        .respond_with(FailOnce {
            requests: Arc::clone(&requests),
            failure,
        })
        .mount(&server)
        .await;
    let llm = Llm::new(llm_config(server.uri()))?;
    let mut deltas = Vec::new();

    let completion = llm
        .complete(
            "system",
            &[user("hi")],
            &[],
            &CancellationToken::new(),
            |delta| deltas.push(delta),
        )
        .await?;

    Ok((requests.load(Ordering::SeqCst), deltas, completion))
}

#[tokio::test]
async fn retries_429_before_streaming() -> Result<()> {
    let (requests, deltas, completion) =
        complete_after_failure(ResponseTemplate::new(429).insert_header("Retry-After", "0"))
            .await?;

    assert_eq!(requests, 2);
    assert_eq!(deltas, vec![Delta::Text("recovered".to_owned())]);
    assert_eq!(completion.message.content.as_deref(), Some("recovered"));
    Ok(())
}

#[tokio::test]
async fn retries_a_stream_that_fails_midway_and_voids_its_output() -> Result<()> {
    let (requests, deltas, completion) = complete_after_failure(stream_response(&[
        json!({"choices":[{"delta":{"content":"partial"},"finish_reason":null}]}),
        json!({"choices":[], "error":{"message":"provider exploded","type":"server_error"}}),
    ]))
    .await?;

    assert_eq!(requests, 2);
    assert_eq!(
        deltas,
        vec![
            Delta::Text("partial".to_owned()),
            Delta::Restart,
            Delta::Text("recovered".to_owned())
        ]
    );
    assert_eq!(completion.message.content.as_deref(), Some("recovered"));
    Ok(())
}

#[test]
fn user_content_uses_strings_until_images_are_present() -> Result<()> {
    let text_message = user("hello");
    let text_json = serde_json::to_value(&text_message)?;
    assert_eq!(text_json["content"], "hello");

    let mixed_message = Message::User {
        content: UserContent::from_parts(vec![
            ContentPart::Text {
                text: "caption".to_owned(),
            },
            ContentPart::ImageUrl {
                image_url: ImageUrl {
                    url: "data:image/png;base64,AA==".to_owned(),
                },
            },
        ]),
    };
    let mixed_json = serde_json::to_value(&mixed_message)?;
    assert!(mixed_json["content"].is_array());
    assert_eq!(
        serde_json::from_value::<Message>(mixed_json)?,
        mixed_message
    );
    Ok(())
}
