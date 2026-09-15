use std::future::Future;
use std::sync::Arc;

use anyhow::{Result, anyhow};
use base64::Engine;
use futures::future::BoxFuture;
use schemars::{JsonSchema, generate::SchemaSettings};
use serde::de::DeserializeOwned;
use serde_json::Value;
use tokio_util::sync::CancellationToken;

use crate::llm::{FunctionSpec, ToolSpec, ToolType};

const MAX_OUTPUT_BYTES: usize = 50 * 1024;
const MAX_OUTPUT_LINES: usize = 2_000;

type ToolHandler =
    dyn Fn(ToolContext, Value) -> BoxFuture<'static, Result<ToolOutput>> + Send + Sync;

pub struct Tool {
    pub name: String,
    pub description: String,
    pub parameters: Value,
    handler: Arc<ToolHandler>,
}

impl Tool {
    pub fn new<A, F, Fut>(
        name: impl Into<String>,
        description: impl Into<String>,
        handler: F,
    ) -> Self
    where
        A: DeserializeOwned + JsonSchema + Send + 'static,
        F: Fn(ToolContext, A) -> Fut + Send + Sync + 'static,
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
        Self::raw(name, description, parameters, move |ctx, arguments| {
            let parsed = serde_json::from_value(arguments)
                .map_err(|error| anyhow!("invalid tool arguments: {error}"))
                .map(|arguments| handler(ctx, arguments));
            Box::pin(async move { parsed?.await })
        })
    }

    pub fn raw<F>(
        name: impl Into<String>,
        description: impl Into<String>,
        parameters: Value,
        handler: F,
    ) -> Self
    where
        F: Fn(ToolContext, Value) -> BoxFuture<'static, Result<ToolOutput>> + Send + Sync + 'static,
    {
        Self {
            name: name.into(),
            description: description.into(),
            parameters,
            handler: Arc::new(handler),
        }
    }

    pub async fn execute(&self, ctx: ToolContext, arguments: Value) -> Result<ToolOutput> {
        (self.handler)(ctx, arguments).await
    }
}

#[derive(Clone)]
pub struct ToolContext {
    pub cancel: CancellationToken,
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

pub fn truncate_output(text: &str) -> String {
    let line_boundary = text
        .match_indices('\n')
        .nth(MAX_OUTPUT_LINES - 1)
        .map_or(text.len(), |(index, _)| index + 1);
    let mut boundary = text.len().min(MAX_OUTPUT_BYTES).min(line_boundary);
    while !text.is_char_boundary(boundary) {
        boundary -= 1;
    }
    if boundary == text.len() {
        return text.to_owned();
    }

    let omitted = &text[boundary..];
    let omitted_lines = omitted.lines().count();
    format!(
        "{}\n[truncated: omitted {omitted_lines} lines, {} bytes]",
        &text[..boundary],
        omitted.len()
    )
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
        let tool = Tool::new::<Arguments, _, _>("echo", "Echo", |_ctx, arguments| async move {
            Ok(ToolOutput::text(arguments.value))
        });
        assert!(tool.parameters.get("$schema").is_none());
        assert!(tool.parameters.get("title").is_none());
        assert!(tool.parameters.get("$defs").is_none());
        assert_eq!(tool.parameters["properties"]["value"]["type"], "string");
    }

    #[tokio::test]
    async fn typed_tool_reports_bad_arguments() {
        let tool = Tool::new::<Arguments, _, _>("echo", "Echo", |_ctx, arguments| async move {
            Ok(ToolOutput::text(arguments.value))
        });
        let error = tool
            .execute(
                ToolContext {
                    cancel: CancellationToken::new(),
                },
                json!({"wrong": true}),
            )
            .await
            .expect_err("invalid arguments should fail");
        assert!(error.to_string().contains("missing field `value`"));
    }

    #[test]
    fn truncates_at_byte_limit_on_utf8_boundary() {
        let text = format!("{}éafter", "a".repeat(MAX_OUTPUT_BYTES - 1));
        let output = truncate_output(&text);
        assert!(output.contains("[truncated: omitted 1 lines, 7 bytes]"));
        assert!(output.is_char_boundary(output.len()));
    }

    #[test]
    fn truncates_at_line_limit() {
        let text = (0..2_005)
            .map(|index| format!("{index}\n"))
            .collect::<String>();
        let output = truncate_output(&text);
        assert!(output.contains("[truncated: omitted 5 lines,"));
        assert_eq!(output.matches('\n').count(), MAX_OUTPUT_LINES + 1);
    }

    #[test]
    fn leaves_short_output_unchanged() {
        assert_eq!(truncate_output("short\noutput"), "short\noutput");
    }
}
