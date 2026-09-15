use std::future::Future;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use base64::Engine;
use futures::future::BoxFuture;
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::{Deserialize, de::DeserializeOwned};
use serde_json::Value;

use crate::chat::Run;
use crate::llm::{FunctionSpec, ToolSpec, ToolType};

pub(crate) mod discord;
mod forge;
mod mcp;
pub(crate) mod repo;
mod shell;

#[derive(Clone, Copy, JsonSchema, Deserialize)]
#[serde(try_from = "String")]
pub(crate) struct Snowflake(#[schemars(with = "String")] u64);

impl Snowflake {
    pub const fn get(self) -> u64 {
        self.0
    }
}

impl TryFrom<String> for Snowflake {
    type Error = String;

    fn try_from(value: String) -> Result<Self, Self::Error> {
        value
            .parse::<u64>()
            .map_err(|_| "Discord ID must be an unsigned integer string".to_owned())
            .and_then(|id| {
                (id != 0)
                    .then_some(Self(id))
                    .ok_or_else(|| "Discord ID must not be zero".to_owned())
            })
    }
}

type ToolHandler = dyn Fn(Value) -> BoxFuture<'static, Result<ToolOutput>> + Send + Sync;

pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    handler: Arc<ToolHandler>,
}

impl Tool {
    pub fn new<A, S, F, Fut>(
        name: impl Into<String>,
        description: impl Into<String>,
        state: S,
        handler: F,
    ) -> Self
    where
        A: DeserializeOwned + JsonSchema + Send + 'static,
        S: Clone + Send + Sync + 'static,
        F: Fn(S, A) -> Fut + Send + Sync + 'static,
        Fut: Future<Output = Result<ToolOutput>> + Send + 'static,
    {
        let settings = SchemaSettings::draft2020_12().with(|settings| {
            settings.inline_subschemas = true;
            settings.meta_schema = None;
        });
        let schema = settings.into_generator().into_root_schema_for::<A>();
        let mut parameters = Value::from(schema);
        if let Some(object) = parameters.as_object_mut() {
            object.remove("$schema");
            object.remove("title");
            object.remove("$defs");
        }
        Self::raw(
            name,
            description,
            parameters,
            state,
            move |state, arguments| {
                let parsed = serde_json::from_value(arguments)
                    .map_err(|error| anyhow!("invalid tool arguments: {error}"))
                    .map(|arguments| handler(state, arguments));
                Box::pin(async move { parsed?.await })
            },
        )
    }

    pub fn raw<S, F>(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        state: S,
        handler: F,
    ) -> Self
    where
        S: Clone + Send + Sync + 'static,
        F: Fn(S, Value) -> BoxFuture<'static, Result<ToolOutput>> + Send + Sync + 'static,
    {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            handler: Arc::new(move |arguments| handler(state.clone(), arguments)),
        }
    }

    pub async fn execute(&self, arguments: Value) -> Result<ToolOutput> {
        (self.handler)(arguments).await
    }
}

#[derive(Debug)]
pub struct ToolOutput {
    pub text: String,
    pub images: Vec<ImageData>,
}

impl ToolOutput {
    pub fn text(text: impl Into<String>) -> Self {
        Self {
            text: text.into(),
            images: Vec::new(),
        }
    }
}

#[derive(Debug)]
pub struct ImageData {
    pub mime_type: String,
    pub base64_data: String,
}

impl ImageData {
    pub fn data_url(&self) -> String {
        format!("data:{};base64,{}", self.mime_type, self.base64_data)
    }

    pub fn from_bytes(mime_type: impl Into<String>, bytes: &[u8]) -> Self {
        Self {
            mime_type: mime_type.into(),
            base64_data: base64::engine::general_purpose::STANDARD.encode(bytes),
        }
    }
}

#[derive(Default)]
pub struct ToolSet {
    tools: Vec<Tool>,
}

impl ToolSet {
    pub fn new(tools: Vec<Tool>) -> Self {
        Self { tools }
    }

    pub fn specs(&self) -> Vec<ToolSpec> {
        self.tools
            .iter()
            .map(|tool| ToolSpec {
                kind: ToolType::Function,
                function: FunctionSpec {
                    name: tool.name.clone(),
                    description: tool.description.clone(),
                    parameters: tool.parameters.clone(),
                },
            })
            .collect()
    }

    pub fn get(&self, name: &str) -> Option<&Tool> {
        self.tools.iter().find(|tool| tool.name == name)
    }
}

pub fn for_run(run: &Arc<Run>) -> ToolSet {
    let mut tools = discord::tools(run);
    tools.extend(forge::tools(run));
    tools.extend(repo::tools(run));
    if run.app.config.shell.enabled {
        tools.extend(shell::tools(run));
    }
    tools.extend(mcp::tools(run));
    ToolSet::new(tools)
}

#[cfg(test)]
mod tests {
    use super::*;
    use schemars::JsonSchema;
    use serde::Deserialize;
    use serde_json::json;

    #[derive(Deserialize, JsonSchema)]
    struct Arguments {
        value: String,
    }

    #[test]
    fn typed_schema_is_inline_and_stripped() {
        let tool =
            Tool::new::<Arguments, _, _, _>("echo", "Echo", (), |_state, arguments| async move {
                Ok(ToolOutput::text(arguments.value))
            });
        assert!(tool.parameters.get("$schema").is_none());
        assert!(tool.parameters.get("title").is_none());
        assert!(tool.parameters.get("$defs").is_none());
        assert_eq!(tool.parameters["properties"]["value"]["type"], "string");
    }

    #[tokio::test]
    async fn typed_tool_reports_bad_arguments() {
        let tool =
            Tool::new::<Arguments, _, _, _>("echo", "Echo", (), |_state, arguments| async move {
                Ok(ToolOutput::text(arguments.value))
            });
        let error = tool
            .execute(json!({"wrong": true}))
            .await
            .expect_err("invalid arguments should fail");
        assert!(error.to_string().contains("missing field `value`"));
    }
}
