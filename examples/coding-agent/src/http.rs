//! Policy-limited HTTP transport available to explicitly granted provider plugins.

use anyhow::{Context, Result};
use reqwest::{Url, header::HeaderMap};
use std::{env, time::Duration};

const MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;
const DEFAULT_ALLOWED_ORIGIN: &str = "http://127.0.0.1:8080";

#[derive(Clone)]
pub(crate) struct Client {
    http: reqwest::Client,
    allowed_origins: Vec<String>,
}

pub(crate) struct Response {
    pub(crate) status: u16,
    pub(crate) body: String,
}

impl Client {
    pub(crate) fn from_env() -> Result<Self> {
        let allowed_origins = match env::var("CODING_AGENT_HTTP_ORIGINS") {
            Ok(origins) => origins
                .split(',')
                .map(str::trim)
                .filter(|origin| !origin.is_empty())
                .map(str::to_owned)
                .collect(),
            Err(env::VarError::NotPresent) => vec![DEFAULT_ALLOWED_ORIGIN.to_owned()],
            Err(error) => return Err(error).context("failed to read CODING_AGENT_HTTP_ORIGINS"),
        };
        Self::new(allowed_origins)
    }

    pub(crate) fn new(allowed_origins: impl IntoIterator<Item = String>) -> Result<Self> {
        let allowed_origins = allowed_origins
            .into_iter()
            .map(|origin| normalized_origin(&origin))
            .collect::<Result<Vec<_>>>()?;
        let http = reqwest::Client::builder()
            .timeout(Duration::from_secs(10 * 60))
            .build()
            .context("failed to build the provider HTTP client")?;
        Ok(Self {
            http,
            allowed_origins,
        })
    }

    pub(crate) async fn post(
        &self,
        url: String,
        headers: Vec<(String, String)>,
        body: String,
    ) -> Result<Response, String> {
        let url = Url::parse(&url).map_err(|error| format!("invalid provider URL: {error}"))?;
        if !self.allows(&url) {
            return Err(format!(
                "provider URL origin `{}` is not allowed",
                url.origin().ascii_serialization()
            ));
        }
        let headers = headers
            .into_iter()
            .map(|(name, value)| {
                let name = name
                    .parse::<reqwest::header::HeaderName>()
                    .map_err(|error| format!("invalid HTTP header name: {error}"))?;
                let value = value
                    .parse::<reqwest::header::HeaderValue>()
                    .map_err(|error| format!("invalid HTTP header value: {error}"))?;
                Ok((name, value))
            })
            .collect::<Result<HeaderMap, String>>()?;
        let response = self
            .http
            .post(url)
            .headers(headers)
            .body(body)
            .send()
            .await
            .map_err(|error| format!("provider HTTP request failed: {error}"))?;
        let status = response.status().as_u16();
        let body = response
            .bytes()
            .await
            .map_err(|error| format!("failed to read provider response: {error}"))?;
        if body.len() > MAX_RESPONSE_BYTES {
            return Err(format!(
                "provider response exceeded the {MAX_RESPONSE_BYTES}-byte limit"
            ));
        }
        let body = String::from_utf8(body.to_vec())
            .map_err(|_| "provider response was not UTF-8".to_owned())?;
        Ok(Response { status, body })
    }

    fn allows(&self, url: &Url) -> bool {
        self.allowed_origins
            .iter()
            .any(|origin| origin == &url.origin().ascii_serialization())
    }
}

fn normalized_origin(value: &str) -> Result<String> {
    let url =
        Url::parse(value).with_context(|| format!("invalid allowed HTTP origin `{value}`"))?;
    Ok(url.origin().ascii_serialization())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn policy_allows_only_explicit_origins() {
        let client = Client::new(["http://127.0.0.1:8080".to_owned()]).unwrap();

        assert!(client.allows(&Url::parse("http://127.0.0.1:8080/v1").unwrap()));
        assert!(!client.allows(&Url::parse("http://127.0.0.1:9000/v1").unwrap()));
        assert!(!client.allows(&Url::parse("https://example.com/v1").unwrap()));
    }
}
