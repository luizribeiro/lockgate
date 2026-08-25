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
    use core::{fmt, time::Duration};

    use bytes::Bytes;
    use http::header::{AUTHORIZATION, CONTENT_TYPE, HeaderName, HeaderValue};
    use http::{HeaderMap, Method, StatusCode};
    use http_body_util::{BodyExt as _, Full};
    use serde::Serialize;
    use wasip3::http::types::{RequestOptions, Response as WasiResponse};
    use wasip3::http_compat::{
        IncomingBody, RequestOptionsExtension, http_from_wasi_response, http_into_wasi_request,
    };

    use super::DEFAULT_MAX_RESPONSE_BYTES;

    /// Stateless builder for mediated guest HTTP requests.
    #[derive(Clone, Copy, Debug)]
    pub struct Client {
        max_response_bytes: usize,
        connect_timeout: Option<Duration>,
        first_byte_timeout: Option<Duration>,
        between_bytes_timeout: Option<Duration>,
    }

    impl Client {
        /// Creates a client with [`DEFAULT_MAX_RESPONSE_BYTES`].
        pub const fn new() -> Self {
            Self {
                max_response_bytes: DEFAULT_MAX_RESPONSE_BYTES,
                connect_timeout: None,
                first_byte_timeout: None,
                between_bytes_timeout: None,
            }
        }

        /// Creates a client with a caller-selected response body limit.
        pub const fn with_max_response_bytes(max_response_bytes: usize) -> Self {
            Self {
                max_response_bytes,
                connect_timeout: None,
                first_byte_timeout: None,
                between_bytes_timeout: None,
            }
        }

        /// Sets the timeout for connecting to the HTTP server.
        pub const fn with_connect_timeout(mut self, timeout: Duration) -> Self {
            self.connect_timeout = Some(timeout);
            self
        }

        /// Sets the timeout for receiving the first byte of the response.
        pub const fn with_first_byte_timeout(mut self, timeout: Duration) -> Self {
            self.first_byte_timeout = Some(timeout);
            self
        }

        /// Sets the timeout between subsequent response-body chunks.
        pub const fn with_between_bytes_timeout(mut self, timeout: Duration) -> Self {
            self.between_bytes_timeout = Some(timeout);
            self
        }

        /// Starts a GET request.
        pub fn get(&self, url: &str) -> RequestBuilder {
            RequestBuilder::new(Method::GET, url, self)
        }

        /// Starts a POST request.
        pub fn post(&self, url: &str) -> RequestBuilder {
            RequestBuilder::new(Method::POST, url, self)
        }
    }

    impl Default for Client {
        fn default() -> Self {
            Self::new()
        }
    }

    /// A request awaiting optional headers/body configuration and dispatch.
    pub struct RequestBuilder {
        method: Method,
        url: String,
        headers: HeaderMap,
        body: Vec<u8>,
        max_response_bytes: usize,
        connect_timeout: Option<Duration>,
        first_byte_timeout: Option<Duration>,
        between_bytes_timeout: Option<Duration>,
        pending_error: Option<Error>,
    }

    impl RequestBuilder {
        fn new(method: Method, url: &str, client: &Client) -> Self {
            Self {
                method,
                url: url.to_string(),
                headers: HeaderMap::new(),
                body: Vec::new(),
                max_response_bytes: client.max_response_bytes,
                connect_timeout: client.connect_timeout,
                first_byte_timeout: client.first_byte_timeout,
                between_bytes_timeout: client.between_bytes_timeout,
                pending_error: None,
            }
        }

        /// Adds a validated HTTP header.
        pub fn header(mut self, name: &str, value: &str) -> Result<Self, Error> {
            self.fail_if_pending()?;
            let name = HeaderName::try_from(name).map_err(Error::InvalidHeaderName)?;
            let value = HeaderValue::try_from(value).map_err(Error::InvalidHeaderValue)?;
            self.headers.insert(name, value);
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
                        self.headers.insert(AUTHORIZATION, value);
                    }
                    Err(error) => self.pending_error = Some(Error::InvalidHeaderValue(error)),
                }
            }
            self
        }

        /// Serializes a JSON request body and sets its content type.
        pub fn json<T: Serialize + ?Sized>(mut self, value: &T) -> Result<Self, Error> {
            self.fail_if_pending()?;
            self.body = serde_json::to_vec(value).map_err(Error::Json)?;
            self.headers
                .insert(CONTENT_TYPE, HeaderValue::from_static("application/json"));
            Ok(self)
        }

        /// Sets an arbitrary request body.
        pub fn body(mut self, body: impl Into<Vec<u8>>) -> Self {
            self.body = body.into();
            self
        }

        /// Sends the request and collects its response under the configured cap.
        pub async fn send(mut self) -> Result<Response, Error> {
            self.fail_if_pending()?;
            let options = self.request_options()?;
            let mut request = http::Request::builder()
                .method(self.method)
                .uri(self.url)
                .body(Full::new(Bytes::from(self.body)))
                .map_err(|error| Error::Request(RequestError::build(error)))?;
            *request.headers_mut() = self.headers;
            if let Some(options) = options {
                request
                    .extensions_mut()
                    .insert(RequestOptionsExtension(options));
            }
            let request = http_into_wasi_request(request)
                .map_err(|error| Error::Request(RequestError::wasi("request conversion", error)))?;
            let response = wasip3::http::client::send(request)
                .await
                .map_err(|error| Error::Request(RequestError::wasi("send", error)))?;
            let response = http_from_wasi_response(response).map_err(|error| {
                Error::Request(RequestError::wasi("response conversion", error))
            })?;
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

        fn request_options(&self) -> Result<Option<RequestOptions>, Error> {
            if self.connect_timeout.is_none()
                && self.first_byte_timeout.is_none()
                && self.between_bytes_timeout.is_none()
            {
                return Ok(None);
            }

            let options = RequestOptions::new();
            apply_timeout("connect", self.connect_timeout, |timeout| {
                options.set_connect_timeout(timeout)
            })?;
            apply_timeout("first-byte", self.first_byte_timeout, |timeout| {
                options.set_first_byte_timeout(timeout)
            })?;
            apply_timeout("between-bytes", self.between_bytes_timeout, |timeout| {
                options.set_between_bytes_timeout(timeout)
            })?;
            Ok(Some(options))
        }
    }

    fn apply_timeout(
        kind: &str,
        timeout: Option<Duration>,
        setter: impl FnOnce(Option<u64>) -> Result<(), wasip3::http::types::RequestOptionsError>,
    ) -> Result<(), Error> {
        let Some(timeout) = timeout else {
            return Ok(());
        };
        let timeout = timeout_nanos(kind, timeout)?;
        setter(Some(timeout)).map_err(|error| {
            Error::Request(RequestError::wasi(
                &alloc::format!("{kind} timeout configuration"),
                error,
            ))
        })
    }

    fn timeout_nanos(kind: &str, timeout: Duration) -> Result<u64, Error> {
        u64::try_from(timeout.as_nanos()).map_err(|_| {
            Error::Request(RequestError(alloc::format!(
                "{kind} timeout {timeout:?} exceeds wasi:http's duration range"
            )))
        })
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
        pub fn body(&self) -> &[u8] {
            &self.body
        }

        /// Returns the collected response body.
        pub fn bytes(&self) -> &[u8] {
            self.body()
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
        Request(RequestError),
        Body(BodyError),
        ResponseTooLarge { limit: usize },
        Utf8(core::str::Utf8Error),
    }

    /// Keeps request transport details outside the stable public API.
    #[derive(Debug)]
    pub struct RequestError(String);

    impl RequestError {
        fn build(error: http::Error) -> Self {
            Self(alloc::format!("could not build HTTP request: {error}"))
        }

        fn wasi(operation: &str, error: impl fmt::Debug) -> Self {
            Self(alloc::format!("wasi:http {operation} returned {error:?}"))
        }
    }

    impl fmt::Display for RequestError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(&self.0)
        }
    }

    impl core::error::Error for RequestError {}

    /// Keeps response transport details outside the stable public API.
    #[derive(Debug)]
    pub struct BodyError(String);

    impl fmt::Display for BodyError {
        fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
            formatter.write_str(&self.0)
        }
    }

    impl core::error::Error for BodyError {}

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

    async fn collect_limited(
        mut body: IncomingBody<WasiResponse>,
        limit: usize,
    ) -> Result<Vec<u8>, Error> {
        let mut bytes = Vec::new();
        while let Some(frame) = body.frame().await {
            let frame = frame.map_err(|error| {
                Error::Body(BodyError(alloc::format!(
                    "wasi:http body stream returned {error:?}"
                )))
            })?;
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
pub use wasm::{BodyError, Client, Error, RequestBuilder, RequestError, Response};
