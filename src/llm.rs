use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{Client, Request, Response, StatusCode};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tokio_util::sync::CancellationToken;
use tracing::{debug, warn};

use crate::config::{LlmConfig, ModelConfig, ProviderKind};

pub(crate) mod anthropic;
mod openai;

const MAX_RETRIES: u32 = 3;
const ERROR_BODY_LIMIT: usize = 2 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const READ_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Message {
    User {
        content: UserContent,
    },
    Assistant(AssistantMessage),
    Tool {
        tool_call_id: String,
        content: String,
    },
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
    // Thinking signatures bind to the exact prompt prefix: clear these when system, tools, or earlier messages change.
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub anthropic_content: Vec<anthropic::ContentBlock>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(untagged)]
pub enum UserContent {
    Text(String),
    Parts(Vec<ContentPart>),
}

impl UserContent {
    pub fn from_parts(parts: Vec<ContentPart>) -> Self {
        match parts.as_slice() {
            [ContentPart::Text { text }] => Self::Text(text.clone()),
            _ => Self::Parts(parts),
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ContentPart {
    Text { text: String },
    ImageUrl { image_url: ImageUrl },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ImageUrl {
    pub url: String,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolCall {
    pub id: String,
    #[serde(rename = "type")]
    pub kind: ToolType,
    pub function: FunctionCall,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub extra_content: Option<Value>,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ToolType {
    Function,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FunctionCall {
    pub name: String,
    pub arguments: String,
}

#[derive(Clone, Debug, PartialEq, Serialize)]
pub struct ToolSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
}

#[derive(Clone, Copy, Debug, PartialEq)]
pub enum ToolChoice {
    Auto,
    None,
}

#[derive(Clone, Debug, PartialEq)]
pub enum Delta {
    Text(String),
    Reasoning(String),
    Restart,
}

#[derive(Clone, Debug, PartialEq)]
pub struct Completion {
    pub message: AssistantMessage,
    pub finish_reason: FinishReason,
    pub usage: Option<Usage>,
}

#[derive(Clone, Debug, PartialEq)]
pub enum FinishReason {
    Stop,
    Length,
    ToolCalls,
    Refusal,
    Other(String),
}

#[derive(Clone, Debug, Default, PartialEq)]
pub struct Usage {
    pub prompt_tokens: u64,
    pub cached_tokens: u64,
    pub completion_tokens: u64,
}

pub struct Llm {
    models: Vec<Arc<Model>>,
    default: Arc<Model>,
}

pub struct Model {
    pub key: String,
    pub config: ModelConfig,
    provider: Arc<Provider>,
}

struct Provider {
    kind: ProviderKind,
    base_url: String,
    api_key: String,
    http: Client,
}

impl Llm {
    pub fn new(config: &LlmConfig) -> Result<Self> {
        let http = Client::builder()
            .connect_timeout(CONNECT_TIMEOUT)
            .read_timeout(READ_TIMEOUT)
            .build()
            .context("failed to build LLM HTTP client")?;
        let models = config
            .providers
            .iter()
            .flat_map(|(name, provider)| {
                let shared = Arc::new(Provider {
                    kind: provider.kind,
                    base_url: provider.base_url.trim_end_matches('/').to_owned(),
                    api_key: provider.api_key.clone(),
                    http: http.clone(),
                });
                provider.models.iter().map(move |model| {
                    Arc::new(Model {
                        key: format!("{name}/{}", model.id),
                        config: model.clone(),
                        provider: Arc::clone(&shared),
                    })
                })
            })
            .collect::<Vec<_>>();
        let default = models
            .iter()
            .find(|model| model.key == config.default_model)
            .cloned()
            .expect("configuration validation guarantees the default model exists");
        Ok(Self { models, default })
    }

    pub fn models(&self) -> &[Arc<Model>] {
        &self.models
    }

    pub fn get(&self, key: &str) -> Option<&Arc<Model>> {
        self.models.iter().find(|model| model.key == key)
    }

    pub fn default_model(&self) -> &Arc<Model> {
        &self.default
    }
}

impl Model {
    pub async fn complete(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[ToolSpec],
        tool_choice: ToolChoice,
        cancel: &CancellationToken,
        mut on_delta: impl FnMut(Delta),
    ) -> Result<Completion> {
        let provider = &*self.provider;
        let prompt = Prompt {
            system,
            messages,
            tools,
            tool_choice,
        };
        let request = match provider.kind {
            ProviderKind::OpenAi => openai::request(provider, &self.config, &prompt),
            ProviderKind::Anthropic => anthropic::request(provider, &self.config, &prompt),
        }
        .build()
        .context("failed to build LLM request")?;

        let mut attempt = 0;
        loop {
            let response = provider.send_with_retries(&request, cancel).await?;
            let parsed = match provider.kind {
                ProviderKind::OpenAi => openai::parse_stream(response, cancel, &mut on_delta).await,
                ProviderKind::Anthropic => {
                    anthropic::parse_stream(response, cancel, &mut on_delta).await
                }
            };
            match parsed {
                Err(error) if !cancel.is_cancelled() && attempt < MAX_RETRIES => {
                    warn!(error = %format!("{error:#}"), attempt, model = %self.key, "LLM stream failed, retrying");
                    on_delta(Delta::Restart);
                    let delay = Duration::from_secs(1 << attempt);
                    attempt += 1;
                    tokio::select! {
                        () = cancel.cancelled() => bail!("LLM request cancelled"),
                        () = tokio::time::sleep(delay) => {}
                    }
                }
                result => {
                    if let Ok(Completion {
                        usage: Some(usage), ..
                    }) = &result
                    {
                        debug!(
                            model = %self.key,
                            prompt_tokens = usage.prompt_tokens,
                            cached_tokens = usage.cached_tokens,
                            completion_tokens = usage.completion_tokens,
                            "LLM usage"
                        );
                    }
                    return result;
                }
            }
        }
    }
}

struct Prompt<'a> {
    system: &'a str,
    messages: &'a [Message],
    tools: &'a [ToolSpec],
    tool_choice: ToolChoice,
}

pub fn clear_replay(messages: &mut [Message]) {
    for message in messages {
        if let Message::Assistant(message) = message {
            message.anthropic_content.clear();
        }
    }
}

impl Provider {
    async fn send_with_retries(
        &self,
        request: &Request,
        cancel: &CancellationToken,
    ) -> Result<Response> {
        let mut attempt = 0;
        loop {
            let request = request
                .try_clone()
                .expect("LLM requests have buffered JSON bodies");
            let sent = tokio::select! {
                () = cancel.cancelled() => bail!("LLM request cancelled"),
                result = self.http.execute(request) => result,
            };
            match sent {
                Ok(response) if response.status().is_success() => return Ok(response),
                Ok(response) if retryable_status(response.status()) && attempt < MAX_RETRIES => {
                    let delay = retry_delay(&response, attempt);
                    attempt += 1;
                    tokio::select! {
                        () = cancel.cancelled() => bail!("LLM request cancelled"),
                        () = tokio::time::sleep(delay) => {}
                    }
                }
                Ok(response) => {
                    return tokio::select! {
                        () = cancel.cancelled() => Err(anyhow!("LLM request cancelled")),
                        error = http_error(response) => error,
                    };
                }
                Err(error)
                    if (error.is_connect() || error.is_timeout()) && attempt < MAX_RETRIES =>
                {
                    let delay = Duration::from_secs(1 << attempt);
                    attempt += 1;
                    tokio::select! {
                        () = cancel.cancelled() => bail!("LLM request cancelled"),
                        () = tokio::time::sleep(delay) => {}
                    }
                }
                Err(error) => return Err(error).context("failed to send LLM request"),
            }
        }
    }
}

fn retryable_status(status: StatusCode) -> bool {
    matches!(
        status,
        StatusCode::REQUEST_TIMEOUT | StatusCode::TOO_MANY_REQUESTS
    ) || status.is_server_error()
}

fn retry_delay(response: &Response, attempt: u32) -> Duration {
    response
        .headers()
        .get(reqwest::header::RETRY_AFTER)
        .and_then(|value| value.to_str().ok())
        .and_then(|value| value.parse().ok())
        .map(Duration::from_secs)
        .map(|delay| delay.min(MAX_RETRY_DELAY))
        .unwrap_or_else(|| Duration::from_secs(1 << attempt))
}

async fn http_error(response: Response) -> Result<Response> {
    let status = response.status();
    let bytes = response
        .bytes()
        .await
        .context("failed to read LLM error response")?;
    let boundary = bytes.len().min(ERROR_BODY_LIMIT);
    let body = String::from_utf8_lossy(&bytes[..boundary]);
    Err(anyhow!("LLM request failed with status {status}: {body}"))
}

#[cfg(test)]
mod live_tests;
#[cfg(test)]
mod tests;
