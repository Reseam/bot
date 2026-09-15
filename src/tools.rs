use std::future::Future;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use base64::Engine;
use futures::future::BoxFuture;
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::de::DeserializeOwned;
use serde_json::Value;

use crate::chat::Run;
use crate::llm::{FunctionSpec, ToolSpec, ToolType};

mod bash;

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
        let mut parameters = Value::from(settings.into_generator().into_root_schema_for::<A>());
        if let Some(object) = parameters.as_object_mut() {
            object.remove("$schema");
            object.remove("title");
            object.remove("$defs");
        }
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            handler: Arc::new(move |arguments| {
                let parsed = serde_json::from_value(arguments)
                    .map_err(|error| anyhow!("invalid tool arguments: {error}"))
                    .map(|arguments| handler(state.clone(), arguments));
                Box::pin(async move { parsed?.await })
            }),
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
    ToolSet::new(vec![bash::tool(run)])
}
