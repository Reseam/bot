use std::collections::BTreeMap;
use std::env;
use std::sync::Arc;

use anyhow::{Context, Result, bail};
use serde_json::{Map, Value, json};
use tokio_util::sync::CancellationToken;

use super::*;
use crate::config::ProviderConfig;
use crate::test_support::user;

fn load_env() -> Result<()> {
    if let Err(error) = dotenvy::dotenv()
        && !error.not_found()
    {
        return Err(error).context("failed to load .env");
    }
    Ok(())
}

fn required(name: &str) -> Result<String> {
    env::var(name).with_context(|| format!("{name} is not set"))
}

fn live_model(
    kind: ProviderKind,
    base_url: String,
    api_key: String,
    id: String,
    extra_body: Map<String, Value>,
) -> Result<Arc<Model>> {
    let config = LlmConfig {
        default_model: format!("live/{id}"),
        providers: BTreeMap::from([(
            "live".to_owned(),
            ProviderConfig {
                kind,
                base_url,
                api_key,
                models: vec![ModelConfig {
                    id,
                    context_window: 128_000,
                    max_output_tokens: 16_000,
                    vision: true,
                    extra_body,
                }],
            },
        )]),
    };
    Ok(Llm::new(&config)?.default_model().clone())
}

struct RoundTrip {
    transcript: Vec<Message>,
    first: Completion,
    second: Completion,
}

fn time_tool() -> ToolSpec {
    ToolSpec {
        name: "get_time".to_owned(),
        description: "Get the current time".to_owned(),
        parameters: json!({
            "type":"object",
            "properties":{"timezone":{"type":"string"}},
            "required":["timezone"],
            "additionalProperties":false
        }),
    }
}

async fn tool_round_trip(model: &Model, system: &str) -> Result<RoundTrip> {
    let tool = time_tool();
    let cancel = CancellationToken::new();
    let mut transcript = vec![user(
        "You must call get_time for Asia/Kolkata before answering. Do not guess the result.",
    )];
    let tools = std::slice::from_ref(&tool);
    let first = model
        .complete(
            system,
            &transcript,
            tools,
            ToolChoice::Auto,
            &cancel,
            |_| {},
        )
        .await?;
    let call = first
        .message
        .tool_calls
        .first()
        .context("model did not call get_time")?
        .clone();
    transcript.push(Message::Assistant(first.message.clone()));
    transcript.push(Message::Tool {
        tool_call_id: call.id,
        content: "The current time is 12:34 IST.".to_owned(),
    });
    let second = model
        .complete(
            system,
            &transcript,
            tools,
            ToolChoice::Auto,
            &cancel,
            |_| {},
        )
        .await?;
    assert!(
        second
            .message
            .content
            .as_ref()
            .is_some_and(|text| !text.is_empty())
    );
    transcript.push(Message::Assistant(second.message.clone()));
    Ok(RoundTrip {
        transcript,
        first,
        second,
    })
}

#[tokio::test]
#[ignore = "calls the configured live OpenAI-compatible endpoint"]
async fn live_openrouter_tool_round_trip() -> Result<()> {
    load_env()?;
    let model = live_model(
        ProviderKind::OpenAi,
        required("LLM_BASE_URL")?,
        required("LLM_API_KEY")?,
        required("LLM_MODEL")?,
        Map::new(),
    )?;
    let RoundTrip { first, .. } =
        tool_round_trip(&model, "Follow the user's tool instruction.").await?;
    if first
        .message
        .reasoning_content
        .as_deref()
        .unwrap_or_default()
        .is_empty()
        && first
            .message
            .reasoning
            .as_deref()
            .unwrap_or_default()
            .is_empty()
    {
        bail!("model did not stream reasoning");
    }
    Ok(())
}

#[tokio::test]
#[ignore = "calls the live Anthropic API"]
async fn live_anthropic_caches_and_replays_across_runs() -> Result<()> {
    load_env()?;
    let model = live_model(
        ProviderKind::Anthropic,
        "https://api.anthropic.com".to_owned(),
        required("ANTHROPIC_API_KEY")?,
        env::var("ANTHROPIC_MODEL").unwrap_or_else(|_| "claude-opus-5-5".to_owned()),
        Map::from_iter([("output_config".to_owned(), json!({"effort": "high"}))]),
    )?;
    let system = format!(
        "Follow the user's tool instruction.\n\n{}",
        (1..=150)
            .map(|rule| format!(
                "Reference rule {rule}: keep answers about time zones short and exact."
            ))
            .collect::<Vec<_>>()
            .join("\n")
    );
    let RoundTrip {
        transcript,
        first,
        second,
    } = tool_round_trip(&model, &system).await?;
    assert!(!first.message.anthropic_content.is_empty());
    assert!(second.usage.context("missing usage")?.cached_tokens > 0);

    let stored = serde_json::to_string(&transcript)?;
    let mut transcript: Vec<Message> = serde_json::from_str(&stored)?;
    assert_eq!(
        transcript[1],
        Message::Assistant(first.message),
        "replay blocks survive storage"
    );
    transcript.push(user("Now say the same time in UTC."));
    let third = model
        .complete(
            &system,
            &transcript,
            &[time_tool()],
            ToolChoice::None,
            &CancellationToken::new(),
            |_| {},
        )
        .await?;
    assert!(third.message.content.is_some_and(|text| !text.is_empty()));
    assert!(third.usage.context("missing usage")?.cached_tokens > 0);
    Ok(())
}
