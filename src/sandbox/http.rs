use std::collections::BTreeMap;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::time::Duration;

use anyhow::{Context, Result, anyhow, bail};
use base64::Engine;
use futures::StreamExt;
use reqwest::dns::{Addrs, Name, Resolve, Resolving};
use reqwest::header::{AUTHORIZATION, HeaderMap, HeaderName, HeaderValue};
use reqwest::{Client, Method, Url, redirect};
use serde::{Deserialize, Serialize};

use crate::chat::Run;
use crate::text::truncate_chars;

const MAX_RESPONSE_BYTES: usize = 10 * 1024 * 1024;
const MAX_REDIRECTS: usize = 10;
const REQUEST_TIMEOUT: Duration = Duration::from_secs(60);
const APPROVAL_BODY_LIMIT: usize = 1_500;
const USER_AGENT: &str = "Reseam-Bot";
const DROPPED_HEADERS: [&str; 4] = ["host", "content-length", "connection", "transfer-encoding"];

#[derive(Deserialize)]
pub struct FetchRequest {
    url: String,
    method: String,
    headers: BTreeMap<String, String>,
    body: Option<String>,
}

#[derive(Serialize)]
pub struct FetchResponse {
    status: u16,
    status_text: String,
    headers: BTreeMap<String, String>,
    body_base64: String,
    url: String,
}

pub struct Http {
    reads: Client,
    writes: Client,
}

impl Http {
    pub fn new() -> Result<Self> {
        let client = |policy| {
            Client::builder()
                .dns_resolver(PublicResolver)
                .redirect(policy)
                .no_proxy()
                .user_agent(USER_AGENT)
                .connect_timeout(Duration::from_secs(20))
                .timeout(REQUEST_TIMEOUT)
                .build()
                .context("failed to build sandbox HTTP client")
        };
        Ok(Self {
            reads: client(redirect::Policy::custom(|attempt| {
                if attempt.previous().len() >= MAX_REDIRECTS {
                    attempt.error("too many redirects")
                } else if let Err(error) = check_host(attempt.url()) {
                    attempt.error(error)
                } else {
                    attempt.follow()
                }
            }))?,
            writes: client(redirect::Policy::none())?,
        })
    }

    pub async fn fetch(&self, run: &Run, request: FetchRequest) -> Result<FetchResponse> {
        let url = Url::parse(&request.url).context("invalid URL")?;
        check_host(&url)?;
        let method =
            Method::from_bytes(request.method.as_bytes()).context("invalid HTTP method")?;
        let is_read = matches!(method, Method::GET | Method::HEAD);
        if !is_read {
            let host = url.host_str().unwrap_or_default();
            let mut action = format!("{method} {url}");
            if let Some(body) = &request.body {
                action.push_str(&format!(
                    "\n```\n{}\n```",
                    truncate_chars(body, APPROVAL_BODY_LIMIT)
                ));
            }
            run.approve(&format!("{method} {host}"), &action).await?;
        }

        let authorization = run
            .app
            .forges
            .iter()
            .find_map(|forge| forge.api_authorization(&url));
        let mut headers = HeaderMap::new();
        for (name, value) in &request.headers {
            let name = HeaderName::from_bytes(name.as_bytes())
                .with_context(|| format!("invalid header name {name}"))?;
            if DROPPED_HEADERS.contains(&name.as_str())
                || (authorization.is_some() && name == AUTHORIZATION)
            {
                continue;
            }
            let value = HeaderValue::from_str(value)
                .with_context(|| format!("invalid value for header {name}"))?;
            headers.append(name, value);
        }
        if let Some(authorization) = authorization {
            headers.insert(
                AUTHORIZATION,
                HeaderValue::from_str(&authorization).context("invalid forge token")?,
            );
        }

        let client = if is_read { &self.reads } else { &self.writes };
        let mut builder = client.request(method, url).headers(headers);
        if let Some(body) = request.body {
            builder = builder.body(body);
        }
        let response = tokio::select! {
            () = run.cancel.cancelled() => bail!("request cancelled"),
            response = builder.send() => response.context("request failed")?,
        };

        let status = response.status();
        let url = response.url().to_string();
        let mut response_headers = BTreeMap::<String, String>::new();
        for (name, value) in response.headers() {
            let value = String::from_utf8_lossy(value.as_bytes());
            response_headers
                .entry(name.as_str().to_owned())
                .and_modify(|existing| {
                    existing.push_str(", ");
                    existing.push_str(&value);
                })
                .or_insert_with(|| value.into_owned());
        }
        let mut body = Vec::new();
        let mut stream = response.bytes_stream();
        while let Some(chunk) = stream.next().await {
            let chunk = chunk.context("failed to read response body")?;
            if body.len() + chunk.len() > MAX_RESPONSE_BYTES {
                bail!(
                    "response is larger than {} MiB",
                    MAX_RESPONSE_BYTES / 1024 / 1024
                );
            }
            body.extend_from_slice(&chunk);
        }
        Ok(FetchResponse {
            status: status.as_u16(),
            status_text: status.canonical_reason().unwrap_or_default().to_owned(),
            headers: response_headers,
            body_base64: base64::engine::general_purpose::STANDARD.encode(body),
            url,
        })
    }
}

fn check_host(url: &Url) -> Result<()> {
    if !matches!(url.scheme(), "http" | "https") {
        bail!("only http and https URLs are allowed");
    }
    let host = url.host_str().context("URL has no host")?;
    match host
        .trim_start_matches('[')
        .trim_end_matches(']')
        .parse::<IpAddr>()
    {
        Ok(ip) if !is_public(ip) => bail!("{ip} is not a public address"),
        _ => Ok(()),
    }
}

struct PublicResolver;

impl Resolve for PublicResolver {
    fn resolve(&self, name: Name) -> Resolving {
        Box::pin(async move {
            let host = name.as_str().to_owned();
            let addresses = tokio::net::lookup_host((host.as_str(), 0))
                .await?
                .filter(|address| is_public(address.ip()))
                .collect::<Vec<SocketAddr>>();
            if addresses.is_empty() {
                return Err(anyhow!("{host} does not resolve to a public address").into());
            }
            Ok(Box::new(addresses.into_iter()) as Addrs)
        })
    }
}

fn is_public(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => is_public_v4(ip),
        IpAddr::V6(ip) => is_public_v6(ip),
    }
}

fn is_public_v4(ip: Ipv4Addr) -> bool {
    let [first, second, ..] = ip.octets();
    !(ip.is_private()
        || ip.is_loopback()
        || ip.is_link_local()
        || ip.is_unspecified()
        || ip.is_broadcast()
        || ip.is_documentation()
        || ip.is_multicast()
        || first == 0
        || first >= 240
        || (first == 100 && (64..128).contains(&second))
        || (first == 198 && (18..20).contains(&second))
        || (first == 192 && second == 0 && ip.octets()[2] == 0))
}

fn is_public_v6(ip: Ipv6Addr) -> bool {
    if let Some(mapped) = ip.to_ipv4_mapped() {
        return is_public_v4(mapped);
    }
    let segments = ip.segments();
    if segments[..6] == [0x64, 0xff9b, 0, 0, 0, 0] {
        let [.., high, low] = segments;
        return is_public_v4(Ipv4Addr::from((u32::from(high) << 16) | u32::from(low)));
    }
    !(ip.is_loopback()
        || ip.is_unspecified()
        || ip.is_unique_local()
        || ip.is_unicast_link_local()
        || ip.is_multicast()
        || (segments[0] == 0x2001 && segments[1] == 0xdb8))
}

#[cfg(test)]
mod tests;
