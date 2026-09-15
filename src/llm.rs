use std::iter::once;
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use reqwest::{Client, Response, StatusCode};
use serde::{Deserialize, Serialize, Serializer};
use serde_json::{Map, Value};
use tokio_util::sync::CancellationToken;
use tracing::warn;

use crate::config::LlmConfig;

mod stream;

use stream::parse_stream;

const MAX_RETRIES: u32 = 3;
const ERROR_BODY_LIMIT: usize = 2 * 1024;
const CONNECT_TIMEOUT: Duration = Duration::from_secs(30);
const READ_TIMEOUT: Duration = Duration::from_secs(300);
const MAX_RETRY_DELAY: Duration = Duration::from_secs(60);

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(tag = "role", rename_all = "lowercase")]
pub enum Message {
    System {
        content: String,
    },
    User {
        content: UserContent,
    },
    Assistant(AssistantMessage),
    Tool {
        tool_call_id: String,
        content: String,
    },
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct AssistantMessage {
    pub content: Option<String>,
    #[serde(default, skip_serializing_if = "Vec::is_empty")]
    pub tool_calls: Vec<ToolCall>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning_content: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reasoning: Option<String>,
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

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct ToolSpec {
    #[serde(rename = "type")]
    pub kind: ToolType,
    pub function: FunctionSpec,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct FunctionSpec {
    pub name: String,
    pub description: String,
    pub parameters: Value,
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
    Other(String),
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
pub struct Usage {
    #[serde(default)]
    pub prompt_tokens: u64,
    #[serde(default)]
    pub completion_tokens: u64,
    #[serde(default)]
    pub total_tokens: u64,
}

pub struct Llm {
    http: Client,
    config: LlmConfig,
}

impl Llm {
    pub fn new(config: LlmConfig) -> Result<Self> {
        Ok(Self {
            http: Client::builder()
                .connect_timeout(CONNECT_TIMEOUT)
                .read_timeout(READ_TIMEOUT)
                .build()
                .context("failed to build LLM HTTP client")?,
            config,
        })
    }

    pub async fn complete(
        &self,
        system: &str,
        messages: &[Message],
        tools: &[ToolSpec],
        cancel: &CancellationToken,
        mut on_delta: impl FnMut(Delta),
    ) -> Result<Completion> {
        let system_message = Message::System {
            content: system.to_owned(),
        };
        let body = RequestBody {
            model: &self.config.model,
            messages: RequestMessages {
                system: &system_message,
                messages,
            },
            stream: true,
            stream_options: StreamOptions {
                include_usage: true,
            },
            max_tokens: self.config.max_output_tokens,
            tools,
            extra_body: &self.config.extra_body,
        };

        let mut attempt = 0;
        loop {
            let response = self.send_with_retries(&body, cancel).await?;
            match parse_stream(response, cancel, &mut on_delta).await {
                Err(error) if !cancel.is_cancelled() && attempt < MAX_RETRIES => {
                    warn!(error = %format!("{error:#}"), attempt, "LLM stream failed, retrying");
                    on_delta(Delta::Restart);
                    let delay = Duration::from_secs(1 << attempt);
                    attempt += 1;
                    tokio::select! {
                        () = cancel.cancelled() => bail!("LLM request cancelled"),
                        () = tokio::time::sleep(delay) => {}
                    }
                }
                result => return result,
            }
        }
    }

    async fn send_with_retries(
        &self,
        body: &RequestBody<'_>,
        cancel: &CancellationToken,
    ) -> Result<Response> {
        let endpoint = format!(
            "{}/chat/completions",
            self.config.base_url.trim_end_matches('/')
        );
        let mut attempt = 0;
        loop {
            let sent = tokio::select! {
                () = cancel.cancelled() => bail!("LLM request cancelled"),
                result = self.http.post(&endpoint).bearer_auth(&self.config.api_key).json(body).send() => result,
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

#[derive(Serialize)]
struct RequestBody<'a> {
    model: &'a str,
    messages: RequestMessages<'a>,
    stream: bool,
    stream_options: StreamOptions,
    max_tokens: u32,
    #[serde(skip_serializing_if = "tool_specs_empty")]
    tools: &'a [ToolSpec],
    #[serde(flatten)]
    extra_body: &'a Map<String, Value>,
}

struct RequestMessages<'a> {
    system: &'a Message,
    messages: &'a [Message],
}

impl Serialize for RequestMessages<'_> {
    fn serialize<S>(&self, serializer: S) -> std::result::Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        serializer.collect_seq(once(self.system).chain(self.messages))
    }
}

#[derive(Serialize)]
struct StreamOptions {
    include_usage: bool,
}

fn tool_specs_empty(tools: &&[ToolSpec]) -> bool {
    tools.is_empty()
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
