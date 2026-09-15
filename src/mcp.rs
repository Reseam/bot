use std::collections::BTreeMap;
use std::sync::Arc;
use std::time::Duration;

use anyhow::{Context, Result, anyhow};
use futures::future::join_all;
use parking_lot::RwLock;
use rmcp::model::Tool as McpTool;
use rmcp::service::{Peer, RoleClient, RunningService};
use serde_json::Value;
use tokio::sync::Mutex as AsyncMutex;
use tracing::{info, warn};

use crate::config::McpServerConfig;
use crate::tools::ToolOutput;

use client::{call_peer, connect_client, convert_result, reconnectable, redact_error};

mod client;

const CONNECT_TIMEOUT: Duration = Duration::from_secs(20);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(5);

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
    pub name: String,
    pub description: String,
    pub input_schema: Value,
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

    pub fn tool(&self, server: &str, name: &str) -> Option<ExposedTool> {
        self.servers
            .get(server)?
            .state
            .read()
            .tools
            .iter()
            .find(|tool| tool.name == name)
            .cloned()
    }

    pub fn tools(&self) -> Vec<ExposedTool> {
        self.servers
            .values()
            .flat_map(|server| server.state.read().tools.clone())
            .collect()
    }

    pub fn status(&self) -> Vec<ServerStatus> {
        self.servers
            .values()
            .map(|server| {
                let state = server.state.read();
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
            let client = server.state.write().client.take();
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
        let state = self.state.read();
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
        let current = self.state.read().generation;
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
            let mut state = self.state.write();
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
            let mut state = self.state.write();
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
        .map(|tool| ExposedTool {
            server: server.to_owned(),
            approve: config.approve.iter().any(|name| name == tool.name.as_ref()),
            name: tool.name.into_owned(),
            description: tool
                .description
                .map(|text| text.into_owned())
                .unwrap_or_default(),
            input_schema: Value::Object((*tool.input_schema).clone()),
            timeout: Duration::from_secs(config.timeout_secs),
        })
        .collect()
}

#[cfg(test)]
mod tests;
