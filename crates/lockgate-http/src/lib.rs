//! Opt-in HTTP client for Lockgate WebAssembly guests.
//!
//! The client is available on `wasm32` and uses the mediated `wasi:http`
//! interface supplied by the Lockgate host. Responses are fully collected
//! under a configurable byte limit before [`RequestBuilder::send`] returns.

#![cfg_attr(target_arch = "wasm32", no_std)]

/// Default maximum response body size: 2 MiB.
pub const DEFAULT_MAX_RESPONSE_BYTES: usize = 2 * 1024 * 1024;

#[cfg(target_arch = "wasm32")]
mod wasm {
    extern crate alloc;

    use alloc::string::{String, ToString};
    use alloc::vec::Vec;
    use core::fmt;

    use http::header::{AUTHORIZATION, CONTENT_TYPE, HeaderName, HeaderValue};
    use http::{HeaderMap, StatusCode};
    use http_body_util::BodyExt as _;
    use serde::Serialize;

    use super::DEFAULT_MAX_RESPONSE_BYTES;

    /// Stateless builder for mediated guest HTTP requests.
    #[derive(Clone, Copy, Debug)]
    pub struct Client {
        max_response_bytes: usize,
    }

    impl Client {
        /// Creates a client with [`DEFAULT_MAX_RESPONSE_BYTES`].
        pub const fn new() -> Self {
            Self {
                max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
            }
        }

        /// Creates a client with a caller-selected response body limit.
        pub const fn with_max_response_bytes(max_response_bytes: usize) -> Self {
            Self { max_response_bytes }
        }

        /// Starts a GET request.
        pub fn get(&self, url: &str) -> RequestBuilder {
            RequestBuilder::new(wasi_fetch::Client::new().get(url), self.max_response_bytes)
        }

        /// Starts a POST request.
        pub fn post(&self, url: &str) -> RequestBuilder {
            RequestBuilder::new(wasi_fetch::Client::new().post(url), self.max_response_bytes)
        }
    }

    impl Default for Client {
        fn default() -> Self {
            Self::new()
        }
    }

    /// A request awaiting optional headers/body configuration and dispatch.
    pub struct RequestBuilder {
        inner: wasi_fetch::RequestBuilder,
        max_response_bytes: usize,
        pending_error: Option<Error>,
    }

    impl RequestBuilder {
        fn new(inner: wasi_fetch::RequestBuilder, max_response_bytes: usize) -> Self {
            Self {
                inner,
                max_response_bytes,
                pending_error: None,
            }
        }

        /// Adds a validated HTTP header.
        pub fn header(mut self, name: &str, value: &str) -> Result<Self, Error> {
            self.fail_if_pending()?;
            let name = HeaderName::try_from(name).map_err(Error::InvalidHeaderName)?;
            let value = HeaderValue::try_from(value).map_err(Error::InvalidHeaderValue)?;
            self.inner = self.inner.header(name, value);
            Ok(self)
        }

        /// Adds `Authorization: Bearer …` when a token is present.
        ///
        /// Invalid header characters are reported by [`Self::json`] when it is
        /// called next, or by [`Self::send`] otherwise.
        pub fn bearer(mut self, token: Option<&str>) -> Self {
            if self.pending_error.is_none()
                && let Some(token) = token
            {
                let value = alloc::format!("Bearer {token}");
                match HeaderValue::try_from(value) {
                    Ok(value) => {
                        self.inner = self.inner.header(AUTHORIZATION, value);
                    }
                    Err(error) => self.pending_error = Some(Error::InvalidHeaderValue(error)),
                }
            }
            self
        }

        /// Serializes a JSON request body and sets its content type.
        pub fn json<T: Serialize + ?Sized>(mut self, value: &T) -> Result<Self, Error> {
            self.fail_if_pending()?;
            let body = serde_json::to_vec(value).map_err(Error::Json)?;
            self.inner = self
                .inner
                .header(CONTENT_TYPE, HeaderValue::from_static("application/json"))
                .body(body);
            Ok(self)
        }

        /// Sets an arbitrary request body.
        pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
            self.inner = self.inner.body(body.into());
            self
        }

        /// Sends the request and collects its response under the configured cap.
        pub async fn send(mut self) -> Result<Response, Error> {
            self.fail_if_pending()?;
            let response = self.inner.send().await.map_err(Error::Request)?;
            let (parts, body) = response.into_parts();
            let body = collect_limited(body, self.max_response_bytes).await?;
            Ok(Response {
                status: parts.status,
                headers: parts.headers,
                body,
            })
        }

        fn fail_if_pending(&mut self) -> Result<(), Error> {
            match self.pending_error.take() {
                Some(error) => Err(error),
                None => Ok(()),
            }
        }
    }

    /// Fully collected HTTP response.
    pub struct Response {
        status: StatusCode,
        headers: HeaderMap,
        body: Vec<u8>,
    }

    impl Response {
        /// Returns the numeric HTTP status code.
        pub fn status(&self) -> u16 {
            self.status.as_u16()
        }

        /// Returns the response headers.
        pub fn headers(&self) -> &HeaderMap {
            &self.headers
        }

        /// Returns the collected response body.
        pub fn bytes(&self) -> &[u8] {
            &self.body
        }

        /// Decodes the collected response body as UTF-8.
        pub fn text(&self) -> Result<String, Error> {
            core::str::from_utf8(&self.body)
                .map(ToString::to_string)
                .map_err(Error::Utf8)
        }
    }

    /// Failure while building, sending, or collecting an HTTP request.
    #[derive(Debug)]
    pub enum Error {
        InvalidHeaderName(http::header::InvalidHeaderName),
        InvalidHeaderValue(http::header::InvalidHeaderValue),
        Json(serde_json::Error),
        Request(wasi_fetch::Error),
        Body(wasi_fetch::Error),
        ResponseTooLarge { limit: usize },
        Utf8(core::str::Utf8Error),
    }

    impl fmt::Display for Error {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            match self {
                Self::InvalidHeaderName(error) => {
                    write!(formatter, "invalid HTTP header name: {error}")
                }
                Self::InvalidHeaderValue(error) => {
                    write!(formatter, "invalid HTTP header value: {error}")
                }
                Self::Json(error) => write!(formatter, "failed to encode JSON request: {error}"),
                Self::Request(error) => write!(formatter, "HTTP request failed: {error}"),
                Self::Body(error) => {
                    write!(formatter, "failed to read HTTP response body: {error}")
                }
                Self::ResponseTooLarge { limit } => {
                    write!(formatter, "HTTP response exceeded the {limit}-byte limit")
                }
                Self::Utf8(error) => {
                    write!(formatter, "HTTP response body was not valid UTF-8: {error}")
                }
            }
        }
    }

    impl core::error::Error for Error {}

    async fn collect_limited(mut body: wasi_fetch::Body, limit: usize) -> Result<Vec<u8>, Error> {
        let mut bytes = Vec::new();
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(Error::Body)?;
            let Ok(chunk) = frame.into_data() else {
                continue;
            };
            if bytes.len().saturating_add(chunk.len()) > limit {
                return Err(Error::ResponseTooLarge { limit });
            }
            bytes.extend_from_slice(&chunk);
        }
        Ok(bytes)
    }
}

#[cfg(target_arch = "wasm32")]
pub use http;
#[cfg(target_arch = "wasm32")]
pub use wasm::{Client, Error, RequestBuilder, Response};
