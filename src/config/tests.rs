use super::*;

fn interpolate(source: &str, values: &[(&str, &str)]) -> Result<toml::Table> {
    let mut table = source.parse::<toml::Table>()?;
    let lookup = |name: &str| {
        values
            .iter()
            .find_map(|(key, value)| (*key == name).then(|| (*value).to_owned()))
            .ok_or(env::VarError::NotPresent)
    };
    interpolate_table(&mut table, &lookup, "")?;
    Ok(table)
}

#[test]
fn interpolates_multiple_references_in_nested_tables_and_arrays() -> Result<()> {
    let table = interpolate(
        r#"
            top = "before-${ONE}-${TWO}-after"
            [nested]
            value = "${TWO}"
            array = ["literal", "${ONE}/suffix"]
        "#,
        &[("ONE", "first"), ("TWO", "second")],
    )?;

    assert_eq!(table["top"].as_str(), Some("before-first-second-after"));
    assert_eq!(table["nested"]["value"].as_str(), Some("second"));
    assert_eq!(table["nested"]["array"][1].as_str(), Some("first/suffix"));
    Ok(())
}

#[test]
fn missing_variable_error_names_variable_and_path() {
    let error = interpolate("[llm]\napi_key = '${MISSING_KEY}'", &[])
        .expect_err("missing interpolation variable should fail");
    let message = error.to_string();
    assert!(message.contains("MISSING_KEY"));
    assert!(message.contains("llm.api_key"));
}

#[test]
fn interpolation_uses_defaults_only_for_missing_variables() -> Result<()> {
    let table = interpolate(
        "present = '${PRESENT:-fallback}'\nmissing = '${MISSING:-fallback}'\nempty = '${EMPTY:-fallback}'\nempty_default = '${UNSET:-}'",
        &[("PRESENT", "value"), ("EMPTY", "")],
    )?;
    assert_eq!(table["present"].as_str(), Some("value"));
    assert_eq!(table["missing"].as_str(), Some("fallback"));
    assert_eq!(table["empty"].as_str(), Some("fallback"));
    assert_eq!(table["empty_default"].as_str(), Some(""));
    Ok(())
}

fn llm(source: &str) -> Result<LlmConfig> {
    Ok(toml::from_str(source)?)
}

const PROVIDERS: &str = r#"
    [providers.openrouter]
    kind = "openai"
    base_url = "https://openrouter.ai/api/v1"
    api_key = "key"
    [[providers.openrouter.models]]
    id = "openai/gpt-5"
    context_window = 400000
    max_output_tokens = 16000

    [providers.anthropic]
    kind = "anthropic"
    base_url = "https://api.anthropic.com"
    api_key = "key"
    [[providers.anthropic.models]]
    id = "claude-opus-5-5"
    context_window = 1000000
    max_output_tokens = 64000
"#;

#[test]
fn default_model_splits_provider_at_the_first_slash() -> Result<()> {
    llm(&format!(
        "default_model = 'openrouter/openai/gpt-5'\n{PROVIDERS}"
    ))?
    .validate()?;
    llm(&format!(
        "default_model = 'anthropic/claude-opus-5-5'\n{PROVIDERS}"
    ))?
    .validate()?;
    for unknown in ["anthropic/openai/gpt-5", "claude-opus-5-5", "openai/gpt-5"] {
        let error = llm(&format!("default_model = '{unknown}'\n{PROVIDERS}"))?
            .validate()
            .expect_err("an unconfigured default model should fail");
        assert!(error.to_string().contains(unknown));
    }
    Ok(())
}
