use std::collections::BTreeMap;

use anyhow::{Context, Result};
use rmcp::model::{CallToolRequestParams, CallToolResult, ContentBlock, ResourceContents};
use serde_json::json;

use super::*;

#[test]
fn result_conversion_joins_text_and_preserves_images_and_resources() -> Result<()> {
    let result = CallToolResult::success(vec![
        ContentBlock::text("first"),
        ContentBlock::image("aGVsbG8=", "image/png"),
        ContentBlock::resource(ResourceContents::text(
            "resource text",
            "file:///result.txt",
        )),
        ContentBlock::resource(
            ResourceContents::blob("AA==", "file:///result.bin")
                .with_mime_type("application/octet-stream"),
        ),
    ]);

    let output = convert_result(result)?;
    assert_eq!(
        output.text,
        "first\n\nresource text\n\n[resource: file:///result.bin, application/octet-stream]"
    );
    assert_eq!(output.images.len(), 1);
    assert_eq!(output.images[0].mime_type, "image/png");
    assert_eq!(output.images[0].base64_data, "aGVsbG8=");
    Ok(())
}

#[tokio::test]
#[ignore = "requires EXA_API_KEY and network access"]
async fn exa_lists_tools_and_runs_search() -> Result<()> {
    dotenvy::dotenv().context("failed to load .env")?;
    let api_key = std::env::var("EXA_API_KEY").context("EXA_API_KEY is not set")?;
    let config = McpServerConfig {
        url: Some("https://mcp.exa.ai/mcp".to_owned()),
        command: None,
        args: Vec::new(),
        env: BTreeMap::new(),
        headers: BTreeMap::from([("x-api-key".to_owned(), api_key)]),
        tools: None,
        approve: Vec::new(),
        timeout_secs: 60,
    };
    let (mut client, tools) = connect_client(&config).await?;
    let tool_names = tools
        .iter()
        .map(|tool| tool.name.as_ref())
        .collect::<Vec<_>>();
    assert!(
        tool_names.contains(&"web_search_exa"),
        "Exa tools: {tool_names:?}"
    );
    assert!(
        tool_names.contains(&"web_fetch_exa"),
        "Exa tools: {tool_names:?}"
    );

    let result = client
        .call_tool(
            CallToolRequestParams::new("web_search_exa").with_arguments(
                json!({"query": "Reseam Android patching", "numResults": 1})
                    .as_object()
                    .context("search arguments are an object")?
                    .clone(),
            ),
        )
        .await?;
    let output = convert_result(result)?;
    assert!(!output.text.is_empty());
    client.close_with_timeout(CLOSE_TIMEOUT).await?;
    Ok(())
}
