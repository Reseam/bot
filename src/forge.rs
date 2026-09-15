use anyhow::{Context, Result};
use base64::Engine;
use reqwest::Url;

use crate::config::{ForgeConfig, ForgeKind};

pub struct Forge {
    pub name: String,
    pub kind: ForgeKind,
    url: Url,
    token: Option<String>,
    pub default_repo: Option<String>,
}

impl Forge {
    pub fn new(name: &str, config: &ForgeConfig) -> Result<Self> {
        Ok(Self {
            name: name.to_owned(),
            kind: config.kind,
            url: Url::parse(&config.url)
                .with_context(|| format!("forges.{name}.url is invalid"))?,
            token: (!config.token.is_empty()).then(|| config.token.clone()),
            default_repo: config.default_repo.clone(),
        })
    }

    pub fn api_base(&self) -> String {
        let base = self.url.as_str().trim_end_matches('/');
        match self.kind {
            ForgeKind::GitHub => base.to_owned(),
            ForgeKind::Forgejo => format!("{base}/api/v1"),
        }
    }

    pub fn spec_url(&self) -> Option<String> {
        match self.kind {
            ForgeKind::GitHub => None,
            ForgeKind::Forgejo => Some(format!(
                "{}/swagger.v1.json",
                self.url.as_str().trim_end_matches('/')
            )),
        }
    }

    pub fn has_token(&self) -> bool {
        self.token.is_some()
    }

    pub fn api_authorization(&self, url: &Url) -> Option<String> {
        let token = self.token.as_ref()?;
        (url.origin() == self.url.origin()).then(|| match self.kind {
            ForgeKind::GitHub => format!("Bearer {token}"),
            ForgeKind::Forgejo => format!("token {token}"),
        })
    }

    pub fn git_authorization(&self, host: &str) -> Option<String> {
        let token = self.token.as_ref()?;
        let forge_host = self.url.host_str()?;
        let (matches, username) = match self.kind {
            ForgeKind::GitHub => (
                forge_host.strip_prefix("api.").unwrap_or(forge_host) == host,
                "x-access-token",
            ),
            ForgeKind::Forgejo => (forge_host == host, "oauth2"),
        };
        matches.then(|| {
            let credentials =
                base64::engine::general_purpose::STANDARD.encode(format!("{username}:{token}"));
            format!("Basic {credentials}")
        })
    }
}

#[cfg(test)]
mod tests;
