use schemars::JsonSchema;
use serde::Deserialize;
use serde_json::json;

use super::*;

#[derive(Deserialize, JsonSchema)]
struct Arguments {
    value: String,
}

#[test]
fn typed_schema_is_inline_and_stripped() {
    let tool = Tool::new(
        "echo",
        "Echo",
        (),
        |_state, arguments: Arguments| async move { Ok(ToolOutput::text(arguments.value)) },
    );
    assert!(tool.parameters.get("$schema").is_none());
    assert!(tool.parameters.get("title").is_none());
    assert!(tool.parameters.get("$defs").is_none());
    assert_eq!(tool.parameters["properties"]["value"]["type"], "string");
}

#[tokio::test]
async fn typed_tool_reports_bad_arguments() {
    let tool = Tool::new(
        "echo",
        "Echo",
        (),
        |_state, arguments: Arguments| async move { Ok(ToolOutput::text(arguments.value)) },
    );
    let error = tool
        .execute(json!({"wrong": true}))
        .await
        .expect_err("invalid arguments should fail");
    assert!(error.to_string().contains("missing field `value`"));
}
