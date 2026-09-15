use std::env;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;

use anyhow::{Context, Result, bail};
use poise::serenity_prelude::{GuildId, RoleId, UserId};
use regex::Regex;
use serde::Deserialize;
use serde_json::{Map, Value};

const DEFAULT_CONFIG_PATH: &str = "config.toml";
static PATTERN: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)\}")
        .expect("the constant interpolation pattern is valid")
});

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Config {
    pub data_dir: PathBuf,
    pub discord: DiscordConfig,
    pub access: AccessConfig,
    pub llm: LlmConfig,
    #[serde(default)]
    pub agent: AgentConfig,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DiscordConfig {
    pub token: String,
    pub guild_id: GuildId,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
pub struct AccessConfig {
    pub owner_ids: Vec<UserId>,
    pub team_role_ids: Vec<RoleId>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct LlmConfig {
    pub base_url: String,
    pub api_key: String,
    pub model: String,
    pub context_window: u32,
    pub max_output_tokens: u32,
    #[serde(default = "default_vision")]
    pub vision: bool,
    #[serde(default)]
    pub extra_body: Map<String, Value>,
}

const fn default_vision() -> bool {
    true
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct AgentConfig {
    pub max_turns: u32,
    pub history_messages: u8,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_turns: 40,
            history_messages: 30,
        }
    }
}

impl Config {
    pub fn load() -> Result<Self> {
        let path = env::var_os("RESEAM_BOT_CONFIG")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from(DEFAULT_CONFIG_PATH));
        Self::load_from(&path)
    }

    fn load_from(path: &Path) -> Result<Self> {
        let source = std::fs::read_to_string(path)
            .with_context(|| format!("failed to read configuration from {}", path.display()))?;
        let mut table = source
            .parse::<toml::Table>()
            .with_context(|| format!("failed to parse configuration from {}", path.display()))?;
        interpolate_table(&mut table, &|name| env::var(name), "")?;
        table
            .try_into()
            .with_context(|| format!("invalid configuration in {}", path.display()))
    }
}

fn interpolate_table(
    table: &mut toml::Table,
    lookup: &impl Fn(&str) -> Result<String, env::VarError>,
    parent: &str,
) -> Result<()> {
    for (key, value) in table {
        let path = if parent.is_empty() {
            key.clone()
        } else {
            format!("{parent}.{key}")
        };
        interpolate_value(value, lookup, &path)?;
    }
    Ok(())
}

fn interpolate_value(
    value: &mut toml::Value,
    lookup: &impl Fn(&str) -> Result<String, env::VarError>,
    path: &str,
) -> Result<()> {
    match value {
        toml::Value::String(text) => {
            let mut output = String::with_capacity(text.len());
            let mut end = 0;
            for captures in PATTERN.captures_iter(text) {
                let whole = captures.get_match();
                let name = &captures[1];
                let replacement = match lookup(name) {
                    Ok(replacement) => replacement,
                    Err(_) => bail!(
                        "environment variable {name} referenced by config key {path} is not set"
                    ),
                };
                output.push_str(&text[end..whole.start()]);
                output.push_str(&replacement);
                end = whole.end();
            }
            output.push_str(&text[end..]);
            *text = output;
        }
        toml::Value::Array(values) => {
            for (index, value) in values.iter_mut().enumerate() {
                interpolate_value(value, lookup, &format!("{path}[{index}]"))?;
            }
        }
        toml::Value::Table(table) => interpolate_table(table, lookup, path)?,
        _ => {}
    }
    Ok(())
}

#[cfg(test)]
mod tests {
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
}
