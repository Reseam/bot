use std::process::Stdio;
use std::time::Duration;

use anyhow::{Context, Result, bail};
use reqwest::header::{HeaderName, HeaderValue};
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock, ResourceContents};
use rmcp::service::{Peer, RoleClient, ServiceError};
use rmcp::transport::{ConfigureCommandExt, TokioChildProcess};
use rmcp::{ServiceExt, model::Tool as McpTool};
use serde_json::{Map, Value};

use super::Client;
use crate::config::McpServerConfig;
use crate::tools::{ImageData, ToolOutput};

pub(super) async fn connect_client(config: &McpServerConfig) -> Result<(Client, Vec<McpTool>)> {
    let client = if let Some(url) = &config.url {
        let headers = config
            .headers
            .iter()
            .map(|(name, value)| {
                Ok((
                    HeaderName::from_bytes(name.as_bytes())
                        .with_context(|| format!("invalid MCP header name `{name}`"))?,
                    HeaderValue::from_str(value)
                        .with_context(|| format!("invalid value for MCP header `{name}`"))?,
                ))
            })
            .collect::<Result<_>>()?;
        let transport_config =
            rmcp::transport::streamable_http_client::StreamableHttpClientTransportConfig::with_uri(
                url.clone(),
            )
            .custom_headers(headers);
        let transport = rmcp::transport::StreamableHttpClientTransport::with_client(
            reqwest::Client::new(),
            transport_config,
        );
        ().serve(transport)
            .await
            .context("failed to initialize HTTP MCP client")?
    } else {
        let command_name = config
            .command
            .as_deref()
            .context("MCP command is missing")?;
        let transport = TokioChildProcess::new(
            tokio::process::Command::new(command_name).configure(|command| {
                command
                    .args(&config.args)
                    .envs(&config.env)
                    .stdin(Stdio::piped())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit())
                    .kill_on_drop(true);
            }),
        )
        .context("failed to start stdio MCP server")?;
        ().serve(transport)
            .await
            .context("failed to initialize stdio MCP client")?
    };
    let tools = client
        .list_all_tools()
        .await
        .context("failed to list MCP tools")?;
    Ok((client, tools))
}

pub(super) async fn call_peer(
    peer: &Peer<RoleClient>,
    tool: &str,
    arguments: Map<String, Value>,
    timeout: Duration,
) -> Result<CallToolResult> {
    let params = CallToolRequestParams::new(tool.to_owned()).with_arguments(arguments);
    tokio::time::timeout(timeout, peer.call_tool(params))
        .await
        .with_context(|| {
            format!(
                "MCP tool call timed out after {} seconds",
                timeout.as_secs()
            )
        })?
        .context("MCP tool call failed")
}

pub(super) fn reconnectable(error: &anyhow::Error) -> bool {
    error.chain().any(|cause| {
        cause.downcast_ref::<ServiceError>().is_some_and(|error| {
            matches!(
                error,
                ServiceError::TransportClosed | ServiceError::TransportSend(_)
            )
        }) || cause
            .to_string()
            .to_ascii_lowercase()
            .contains("session expired")
    })
}

pub(super) fn convert_result(result: CallToolResult) -> Result<ToolOutput> {
    let mut text = Vec::new();
    let mut images = Vec::new();
    for content in result.content {
        match content {
            ContentBlock::Text(content) => text.push(content.text),
            ContentBlock::Image(image) => images.push(ImageData {
                mime_type: image.mime_type,
                base64_data: image.data,
            }),
            ContentBlock::Resource(resource) => match resource.resource {
                ResourceContents::TextResourceContents { text: value, .. } => text.push(value),
                ResourceContents::BlobResourceContents { uri, mime_type, .. } => {
                    text.push(format!(
                        "[resource: {uri}, {}]",
                        mime_type.as_deref().unwrap_or("unknown MIME type")
                    ))
                }
                _ => text.push("[resource: unknown]".to_owned()),
            },
            ContentBlock::ResourceLink(resource) => text.push(format!(
                "[resource: {}, {}]",
                resource.uri,
                resource.mime_type.as_deref().unwrap_or("unknown MIME type")
            )),
            ContentBlock::Audio(audio) => text.push(format!("[audio: {}]", audio.mime_type)),
            _ => text.push("[unsupported MCP content]".to_owned()),
        }
    }
    let text = text.join("\n\n");
    if result.is_error == Some(true) {
        bail!(if text.is_empty() {
            "MCP tool returned an error".to_owned()
        } else {
            text
        });
    }
    Ok(ToolOutput { text, images })
}

pub(super) fn redact_error(config: &McpServerConfig, error: &str) -> String {
    config
        .url
        .iter()
        .chain(config.headers.values())
        .chain(config.env.values())
        .filter(|secret| !secret.is_empty())
        .fold(error.to_owned(), |message, secret| {
            message.replace(secret, "[redacted]")
        })
}
