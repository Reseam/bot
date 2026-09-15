use std::sync::Arc;

use anyhow::Result;
use serde_json::Value;

use super::{Tool, ToolOutput};
use crate::chat::Run;
use crate::mcp::ExposedTool;
use crate::text::truncate_chars;

pub fn tools(run: &Arc<Run>) -> Vec<Tool> {
    run.app
        .mcp
        .tools()
        .into_iter()
        .map(|tool| {
            Tool::raw(
                tool.name.clone(),
                tool.description.clone(),
                tool.parameters.clone(),
                (run.clone(), tool),
                |state, arguments| Box::pin(call(state, arguments)),
            )
        })
        .collect()
}

async fn call(state: (Arc<Run>, ExposedTool), arguments: Value) -> Result<ToolOutput> {
    let (run, tool) = state;
    if tool.approve {
        let preview = truncate_chars(&arguments.to_string(), 1_500);
        run.approve(&tool.name, &preview).await?;
    }
    run.app
        .mcp
        .call(&tool.server, &tool.original_name, arguments, tool.timeout)
        .await
}
