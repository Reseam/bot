use std::env;

use anyhow::{Context, Result, bail};
use serde_json::{Map, json};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::test_support::user;

#[tokio::test]
#[ignore = "calls the configured live OpenAI-compatible endpoint"]
async fn live_openrouter_tool_round_trip() -> Result<()> {
    if let Err(error) = dotenvy::dotenv()
        && !error.not_found()
    {
        return Err(error).context("failed to load .env");
    }
    let config = LlmConfig {
        base_url: env::var("LLM_BASE_URL").context("LLM_BASE_URL is not set")?,
        api_key: env::var("LLM_API_KEY").context("LLM_API_KEY is not set")?,
        model: env::var("LLM_MODEL").context("LLM_MODEL is not set")?,
        context_window: 128_000,
        max_output_tokens: 16_000,
        vision: true,
        extra_body: Map::new(),
    };
    let llm = Llm::new(config)?;
    let tool = ToolSpec {
        kind: ToolType::Function,
        function: FunctionSpec {
            name: "get_time".to_owned(),
            description: "Get the current time".to_owned(),
            parameters: json!({
                "type":"object",
                "properties":{"timezone":{"type":"string"}},
                "required":["timezone"],
                "additionalProperties":false
            }),
        },
    };
    let cancel = CancellationToken::new();
    let mut transcript = vec![user(
        "You must call get_time for Asia/Kolkata before answering. Do not guess the result.",
    )];
    let first = llm
        .complete(
            "Follow the user's tool instruction.",
            &transcript,
            std::slice::from_ref(&tool),
            &cancel,
            |_| {},
        )
        .await?;
    let AssistantMessage {
        tool_calls,
        reasoning_content,
        reasoning,
        ..
    } = &first.message;
    if reasoning_content.as_deref().unwrap_or_default().is_empty()
        && reasoning.as_deref().unwrap_or_default().is_empty()
    {
        bail!("model did not stream reasoning");
    }
    let call = tool_calls
        .first()
        .context("model did not call get_time")?
        .clone();
    transcript.push(Message::Assistant(first.message.clone()));
    transcript.push(Message::Tool {
        tool_call_id: call.id,
        content: "The current time is 12:34 IST.".to_owned(),
    });

    let second = llm
        .complete(
            "Follow the user's tool instruction.",
            &transcript,
            std::slice::from_ref(&tool),
            &cancel,
            |_| {},
        )
        .await?;

    assert!(second.message.content.is_some_and(|text| !text.is_empty()));
    Ok(())
}
