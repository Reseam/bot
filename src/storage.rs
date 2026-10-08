use std::time::{Duration, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use hmac::{Hmac, KeyInit, Mac};
use reqwest::{Client, Method};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::config::StorageConfig;

const UPLOAD_TTL: Duration = Duration::from_secs(15 * 60);
pub const LINK_TTL: Duration = Duration::from_secs(24 * 60 * 60);

pub struct Storage {
    config: StorageConfig,
    host: String,
    http: Client,
}

#[derive(Serialize)]
pub struct Share {
    pub upload: String,
    pub download: String,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct ListBucketResult {
    #[serde(default)]
    contents: Vec<Object>,
    is_truncated: bool,
    next_continuation_token: Option<String>,
}

#[derive(Deserialize)]
#[serde(rename_all = "PascalCase")]
struct Object {
    key: String,
    last_modified: String,
}

impl Storage {
    pub fn new(config: StorageConfig) -> Result<Self> {
        let host = config
            .endpoint
            .strip_prefix("https://")
            .expect("configuration validation guarantees an https endpoint")
            .trim_end_matches('/')
            .to_owned();
        Ok(Self {
            config,
            host,
            http: Client::builder()
                .connect_timeout(Duration::from_secs(30))
                .timeout(Duration::from_secs(60))
                .build()
                .context("failed to build storage HTTP client")?,
        })
    }

    pub fn share(&self, conversation: i64, name: &str) -> Share {
        let now = SystemTime::now();
        let millis = now
            .duration_since(UNIX_EPOCH)
            .expect("the clock is after 1970")
            .as_millis();
        let key = format!("{}{conversation}/{millis}/{name}", self.config.prefix);
        Share {
            upload: self.presign(&Method::PUT, &key, &[], UPLOAD_TTL, now),
            download: self.presign(&Method::GET, &key, &[], LINK_TTL, now),
        }
    }

    pub async fn sweep(&self, older_than: Duration) -> Result<usize> {
        let cutoff = SystemTime::now() - older_than;
        let mut removed = 0;
        let mut token: Option<String> = None;
        loop {
            let mut query = vec![("list-type", "2"), ("prefix", self.config.prefix.as_str())];
            if let Some(token) = &token {
                query.push(("continuation-token", token));
            }
            let url = self.presign(&Method::GET, "", &query, UPLOAD_TTL, SystemTime::now());
            let body = self
                .http
                .get(url)
                .send()
                .await
                .and_then(reqwest::Response::error_for_status)
                .context("failed to list shared files")?
                .text()
                .await
                .context("failed to read the shared file listing")?;
            let listing: ListBucketResult =
                quick_xml::de::from_str(&body).context("invalid shared file listing")?;
            for object in listing.contents {
                let modified = humantime::parse_rfc3339_weak(&object.last_modified)
                    .with_context(|| format!("invalid modification time for {}", object.key))?;
                if modified < cutoff {
                    let url = self.presign(
                        &Method::DELETE,
                        &object.key,
                        &[],
                        UPLOAD_TTL,
                        SystemTime::now(),
                    );
                    self.http
                        .delete(url)
                        .send()
                        .await
                        .and_then(reqwest::Response::error_for_status)
                        .with_context(|| format!("failed to delete {}", object.key))?;
                    removed += 1;
                }
            }
            match listing.next_continuation_token {
                Some(next) if listing.is_truncated => token = Some(next),
                _ => return Ok(removed),
            }
        }
    }

    fn presign(
        &self,
        method: &Method,
        key: &str,
        query: &[(&str, &str)],
        expires: Duration,
        now: SystemTime,
    ) -> String {
        let timestamp = humantime::format_rfc3339_seconds(now)
            .to_string()
            .replace(['-', ':'], "");
        let day = &timestamp[..8];
        let scope = format!("{day}/{}/s3/aws4_request", self.config.region);
        let path = if key.is_empty() {
            format!("/{}", encode(&self.config.bucket, false))
        } else {
            format!(
                "/{}/{}",
                encode(&self.config.bucket, false),
                encode(key, false)
            )
        };
        let credential = format!("{}/{scope}", self.config.access_key_id);
        let expires = expires.as_secs().to_string();
        let mut parameters = vec![
            ("X-Amz-Algorithm", "AWS4-HMAC-SHA256"),
            ("X-Amz-Credential", credential.as_str()),
            ("X-Amz-Date", timestamp.as_str()),
            ("X-Amz-Expires", expires.as_str()),
            ("X-Amz-SignedHeaders", "host"),
        ];
        parameters.extend_from_slice(query);
        let mut encoded = parameters
            .iter()
            .map(|(name, value)| (encode(name, true), encode(value, true)))
            .collect::<Vec<_>>();
        encoded.sort();
        let canonical_query = encoded
            .iter()
            .map(|(name, value)| format!("{name}={value}"))
            .collect::<Vec<_>>()
            .join("&");
        let canonical = format!(
            "{method}\n{path}\n{canonical_query}\nhost:{}\n\nhost\nUNSIGNED-PAYLOAD",
            self.host
        );
        let to_sign = format!(
            "AWS4-HMAC-SHA256\n{timestamp}\n{scope}\n{}",
            hex(&Sha256::digest(canonical.as_bytes()))
        );
        let signing_key = [day, &self.config.region, "s3", "aws4_request"]
            .iter()
            .fold(
                format!("AWS4{}", self.config.secret_access_key).into_bytes(),
                |key, part| hmac(&key, part),
            );
        let signature = hex(&hmac(&signing_key, &to_sign));
        format!(
            "https://{}{path}?{canonical_query}&X-Amz-Signature={signature}",
            self.host
        )
    }
}

fn hmac(key: &[u8], data: &str) -> Vec<u8> {
    let mut mac = Hmac::<Sha256>::new_from_slice(key).expect("HMAC accepts keys of any length");
    mac.update(data.as_bytes());
    mac.finalize().into_bytes().to_vec()
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn encode(value: &str, encode_slash: bool) -> String {
    value
        .bytes()
        .map(|byte| {
            if byte.is_ascii_alphanumeric()
                || b"-_.~".contains(&byte)
                || (byte == b'/' && !encode_slash)
            {
                char::from(byte).to_string()
            } else {
                format!("%{byte:02X}")
            }
        })
        .collect()
}

#[cfg(test)]
mod live_tests;
#[cfg(test)]
mod tests;
