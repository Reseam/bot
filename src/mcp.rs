use std::collections::BTreeMap;
use std::sync::{Arc, RwLock};
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use futures::future::join_all;
use rmcp::model::Tool as McpTool;
use rmcp::service::{Peer, RoleClient, RunningService};
use serde_json::Value;
use tokio::sync::Mutex as AsyncMutex;
use tracing::{info, warn};

use crate::config::McpServerConfig;
use crate::text::truncate_chars;
use crate::tools::ToolOutput;

use client::{call_peer, connect_client, convert_result, reconnectable, redact_error};

mod client;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);
const TOOL_NAME_LIMIT: usize = 64;
const SCHEMA_DESCRIPTION_LIMIT: usize = 1_000;

type Client = RunningService<RoleClient, ()>;

pub struct Mcp {
    servers: BTreeMap<String, Arc<Server>>,
}

struct Server {
    name: String,
    config: McpServerConfig,
    state: RwLock<ServerState>,
    reconnect: AsyncMutex<()>,
}

#[derive(Default)]
struct ServerState {
    client: Option<Client>,
    tools: Vec<ExposedTool>,
    error: Option<String>,
    generation: u64,
}

#[derive(Clone)]
pub struct ExposedTool {
    pub server: String,
    pub original_name: String,
    pub name: String,
    pub description: String,
    pub parameters: Value,
    pub approve: bool,
    pub timeout: Duration,
}

pub struct ServerStatus {
    pub name: String,
    pub transport: &'static str,
    pub tools: Vec<String>,
    pub error: Option<String>,
}

impl Mcp {
    pub async fn connect(configs: &BTreeMap<String, McpServerConfig>) -> Self {
        let servers = configs
            .iter()
            .map(|(name, config)| {
                (
                    name.clone(),
                    Arc::new(Server {
                        name: name.clone(),
                        config: config.clone(),
                        state: RwLock::new(ServerState::default()),
                        reconnect: AsyncMutex::new(()),
                    }),
                )
            })
            .collect::<BTreeMap<_, _>>();
        let mcp = Self { servers };
        join_all(mcp.servers.values().map(|server| async move {
            if let Err(error) = server.reconnect().await {
                warn!(server = %server.name, error = %error, "MCP server unavailable");
            }
        }))
        .await;
        mcp
    }

    pub fn tools(&self) -> Vec<ExposedTool> {
        self.servers
            .values()
            .flat_map(|server| {
                server
                    .state
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner)
                    .tools
                    .clone()
            })
            .collect()
    }

    pub fn status(&self) -> Vec<ServerStatus> {
        self.servers
            .values()
            .map(|server| {
                let state = server
                    .state
                    .read()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                ServerStatus {
                    name: server.name.clone(),
                    transport: server.transport(),
                    tools: state.tools.iter().map(|tool| tool.name.clone()).collect(),
                    error: state.error.clone(),
                }
            })
            .collect()
    }

    pub async fn reconnect(&self, name: &str) -> Result<()> {
        let server = self
            .servers
            .get(name)
            .with_context(|| format!("unknown MCP server `{name}`"))?;
        server.reconnect().await
    }

    pub async fn call(
        &self,
        server_name: &str,
        tool: &str,
        arguments: Value,
        timeout: Duration,
    ) -> Result<ToolOutput> {
        let server = self
            .servers
            .get(server_name)
            .with_context(|| format!("unknown MCP server `{server_name}`"))?;
        let arguments = arguments
            .as_object()
            .cloned()
            .context("MCP tool arguments must be a JSON object")?;
        let (mut peer, mut generation) = server.peer()?;
        if peer.is_transport_closed() {
            server.reconnect_if_current(generation).await?;
            (peer, generation) = server.peer()?;
        }

        let first = call_peer(&peer, tool, arguments.clone(), timeout).await;
        let result = match first {
            Err(error) if reconnectable(&error) => {
                server.reconnect_if_current(generation).await?;
                let (peer, _) = server.peer()?;
                call_peer(&peer, tool, arguments, timeout).await
            }
            result => result,
        };
        result
            .and_then(convert_result)
            .map_err(|error| anyhow!(redact_error(&server.config, &format!("{error:#}"))))
    }

    pub async fn shutdown(&self) {
        join_all(self.servers.values().map(|server| async move {
            let client = server
                .state
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .client
                .take();
            if let Some(mut client) = client
                && let Err(error) = client.close_with_timeout(CLOSE_TIMEOUT).await
            {
                warn!(server = %server.name, %error, "failed to close MCP client");
            }
        }))
        .await;
    }
}

impl Server {
    fn peer(&self) -> Result<(Peer<RoleClient>, u64)> {
        let state = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        let peer = state
            .client
            .as_ref()
            .map(|client| client.peer().clone())
            .with_context(|| {
                state
                    .error
                    .clone()
                    .unwrap_or_else(|| "server unavailable".into())
            })?;
        Ok((peer, state.generation))
    }

    fn transport(&self) -> &'static str {
        if self.config.url.is_some() {
            "streamable HTTP"
        } else {
            "stdio"
        }
    }

    async fn reconnect_if_current(&self, generation: u64) -> Result<()> {
        let _guard = self.reconnect.lock().await;
        let current = self
            .state
            .read()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .generation;
        if current != generation {
            return Ok(());
        }
        self.connect_and_replace().await
    }

    async fn reconnect(&self) -> Result<()> {
        let _guard = self.reconnect.lock().await;
        self.connect_and_replace().await
    }

    async fn connect_and_replace(&self) -> Result<()> {
        let connected = tokio::time::timeout(CONNECT_TIMEOUT, connect_client(&self.config)).await;
        let (client, tools) = match connected {
            Ok(Ok(connected)) => connected,
            Ok(Err(error)) => return self.mark_failed(error).await,
            Err(_) => {
                return self
                    .mark_failed(anyhow!("connection timed out after 20 seconds"))
                    .await;
            }
        };
        let exposed = expose_tools(&self.name, &self.config, tools);
        let names = exposed
            .iter()
            .map(|tool| tool.name.as_str())
            .collect::<Vec<_>>();
        info!(server = %self.name, transport = self.transport(), tools = ?names, "MCP server connected");

        let old = {
            let mut state = self
                .state
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let old = state.client.replace(client);
            state.tools = exposed;
            state.error = None;
            state.generation = state.generation.wrapping_add(1);
            old
        };
        if let Some(mut old) = old
            && let Err(error) = old.close_with_timeout(CLOSE_TIMEOUT).await
        {
            warn!(server = %self.name, %error, "failed to close replaced MCP client");
        }
        Ok(())
    }

    async fn mark_failed(&self, error: anyhow::Error) -> Result<()> {
        let error = redact_error(&self.config, &format!("{error:#}"));
        let old = {
            let mut state = self
                .state
                .write()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            let old = state.client.take();
            state.tools.clear();
            state.error = Some(error.clone());
            state.generation = state.generation.wrapping_add(1);
            old
        };
        if let Some(mut old) = old
            && let Err(close_error) = old.close_with_timeout(CLOSE_TIMEOUT).await
        {
            warn!(server = %self.name, error = %close_error, "failed to close unavailable MCP client");
        }
        Err(anyhow!(error))
    }
}

fn expose_tools(server: &str, config: &McpServerConfig, tools: Vec<McpTool>) -> Vec<ExposedTool> {
    tools
        .into_iter()
        .filter(|tool| {
            config
                .tools
                .as_ref()
                .is_none_or(|allowed| allowed.iter().any(|name| name == tool.name.as_ref()))
        })
        .map(|tool| {
            let original_name = tool.name.into_owned();
            let name = sanitize_tool_name(server, &original_name);
            let schema = Value::Object((*tool.input_schema).clone());
            let (parameters, schema_note) = normalize_schema(schema);
            let mut description = format!(
                "[{server}] {}",
                tool.description
                    .as_deref()
                    .unwrap_or("No description provided.")
            );
            if let Some(note) = schema_note {
                description.push_str(&format!(" Original input schema was not an object: {note}"));
            }
            ExposedTool {
                server: server.to_owned(),
                original_name: original_name.clone(),
                name,
                description,
                parameters,
                approve: config.approve.contains(&original_name),
                timeout: Duration::from_secs(config.timeout_secs),
            }
        })
        .collect()
}

fn normalize_schema(schema: Value) -> (Value, Option<String>) {
    if schema.is_object() {
        return (schema, None);
    }
    let note = truncate_chars(&schema.to_string(), SCHEMA_DESCRIPTION_LIMIT);
    (
        serde_json::json!({"type": "object", "properties": {}}),
        Some(note),
    )
}

fn sanitize_tool_name(server: &str, tool: &str) -> String {
    let original = format!("{server}_{tool}");
    let sanitized = original
        .chars()
        .map(|character| {
            if character.is_ascii_alphanumeric() || matches!(character, '_' | '-') {
                character
            } else {
                '_'
            }
        })
        .collect::<String>();
    if sanitized.len() <= TOOL_NAME_LIMIT {
        return sanitized;
    }
    let hash = original.bytes().fold(0x811c9dc5_u32, |hash, byte| {
        (hash ^ u32::from(byte)).wrapping_mul(0x01000193)
    });
    format!("{}_{hash:08x}", &sanitized[..TOOL_NAME_LIMIT - 9])
}

#[cfg(test)]
mod tests;
