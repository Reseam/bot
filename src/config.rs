use std::collections::BTreeMap;
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
    Regex::new(r"\$\{([A-Za-z_][A-Za-z0-9_]*)(?::-([^}]*))?\}")
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
    #[serde(default)]
    pub forges: BTreeMap<String, ForgeConfig>,
    #[serde(default)]
    pub shell: ShellConfig,
    #[serde(default)]
    pub mcp: BTreeMap<String, McpServerConfig>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct McpServerConfig {
    pub url: Option<String>,
    pub command: Option<String>,
    #[serde(default)]
    pub args: Vec<String>,
    #[serde(default)]
    pub env: BTreeMap<String, String>,
    #[serde(default)]
    pub headers: BTreeMap<String, String>,
    pub tools: Option<Vec<String>>,
    #[serde(default)]
    pub approve: Vec<String>,
    #[serde(default = "default_mcp_timeout_secs")]
    pub timeout_secs: u64,
}

const fn default_mcp_timeout_secs() -> u64 {
    60
}

#[derive(Clone, Copy, Deserialize, Eq, PartialEq)]
#[serde(rename_all = "lowercase")]
pub enum ForgeKind {
    GitHub,
    Forgejo,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ForgeConfig {
    pub kind: ForgeKind,
    pub url: String,
    pub token: String,
    pub default_repo: Option<String>,
}

#[derive(Deserialize)]
#[serde(default, deny_unknown_fields)]
pub struct ShellConfig {
    pub enabled: bool,
    pub timeout_secs: u64,
}

impl Default for ShellConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            timeout_secs: 120,
        }
    }
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
    pub conversation_retention_days: u32,
    pub compaction_reserve_tokens: u64,
    pub keep_recent_tokens: u64,
}

impl Default for AgentConfig {
    fn default() -> Self {
        Self {
            max_turns: 40,
            history_messages: 30,
            conversation_retention_days: 30,
            compaction_reserve_tokens: 16_384,
            keep_recent_tokens: 20_000,
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
        let config: Self = table
            .try_into()
            .with_context(|| format!("invalid configuration in {}", path.display()))?;
        if !(1..=600).contains(&config.shell.timeout_secs) {
            bail!("shell.timeout_secs must be between 1 and 600")
        }
        for (name, server) in &config.mcp {
            server.validate(name)?;
        }
        Ok(config)
    }
}

impl McpServerConfig {
    fn validate(&self, name: &str) -> Result<()> {
        if self.url.is_some() == self.command.is_some() {
            bail!("mcp.{name} must set exactly one of url or command")
        }
        if self.timeout_secs == 0 {
            bail!("mcp.{name}.timeout_secs must be greater than zero")
        }
        Ok(())
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
                let replacement = match (lookup(name), captures.get(2)) {
                    (Ok(replacement), Some(default)) if replacement.is_empty() => {
                        default.as_str().to_owned()
                    }
                    (Ok(replacement), _) => replacement,
                    (Err(_), default) => default.map_or_else(
                        || {
                            Err(anyhow::anyhow!(
                                "environment variable {name} referenced by config key {path} is not set"
                            ))
                        },
                        |value| Ok(value.as_str().to_owned()),
                    )?,
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
mod tests;
