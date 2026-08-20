//! Exact outbound HTTP origin scopes.

use alloc::{format, string::String};
use core::{fmt, str::FromStr};

use url::Url;

use crate::{Scope, ScopeError, ScopeRepr};

/// One exact HTTP origin authorized for outbound requests.
///
/// An origin is the authority of an HTTP(S) URL, so a base URL such as
/// `https://api.example.com/v1` resolves to `https://api.example.com:443`.
/// Host matching is case-insensitive; userinfo and wildcard hosts are rejected.
#[derive(Clone, PartialEq, Eq)]
pub struct HttpOrigin {
    scheme: &'static str,
    host: String,
    port: u16,
}

impl fmt::Debug for HttpOrigin {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_tuple("HttpOrigin")
            .field(&self.canonical())
            .finish()
    }
}

impl FromStr for HttpOrigin {
    type Err = ScopeError;

    fn from_str(value: &str) -> Result<Self, Self::Err> {
        let url = Url::parse(value).map_err(|_| ScopeError::unknown(value))?;
        let scheme = match url.scheme() {
            "http" => "http",
            "https" => "https",
            _ => return Err(ScopeError::unknown(value)),
        };
        if !url.username().is_empty() || url.password().is_some() {
            return Err(ScopeError::unknown(value));
        }
        let host = url
            .host_str()
            .ok_or_else(|| ScopeError::unknown(value))?
            .to_ascii_lowercase();
        let port = url
            .port_or_known_default()
            .ok_or_else(|| ScopeError::unknown(value))?;
        Ok(Self { scheme, host, port })
    }
}

impl ScopeRepr for HttpOrigin {
    fn canonical(&self) -> String {
        format!("{}://{}:{}", self.scheme, self.host, self.port)
    }
}

impl Scope for HttpOrigin {
    fn contains(&self, inner: &Self) -> bool {
        self == inner
    }

    fn intersect(&self, other: &Self) -> Option<Self> {
        (self == other).then(|| self.clone())
    }
}

#[cfg(test)]
mod tests {
    use core::str::FromStr;

    use super::HttpOrigin;
    use crate::{Scope, ScopeRepr, check_scope_laws};

    #[test]
    fn http_origin_samples_obey_scope_laws() {
        check_scope_laws([
            HttpOrigin::from_str("http://example.com").unwrap(),
            HttpOrigin::from_str("https://example.com").unwrap(),
            HttpOrigin::from_str("https://example.com:8443").unwrap(),
            HttpOrigin::from_str("https://other.example").unwrap(),
        ])
        .unwrap();
    }

    #[test]
    fn http_origin_parsing_is_exact_and_canonical() {
        let origin = HttpOrigin::from_str("HTTPS://EXAMPLE.COM").unwrap();
        assert_eq!(origin.canonical(), "https://example.com:443");
        assert_eq!(HttpOrigin::from_str(&origin.canonical()).unwrap(), origin);
        assert!(origin.contains(&HttpOrigin::from_str("https://example.com:443").unwrap()));
        assert!(!origin.contains(&HttpOrigin::from_str("https://example.com:444").unwrap()));

        for base_url in [
            "https://example.com/path",
            "https://example.com?query",
            "https://example.com#fragment",
        ] {
            assert_eq!(
                HttpOrigin::from_str(base_url).unwrap().canonical(),
                "https://example.com:443"
            );
        }

        assert_eq!(
            HttpOrigin::from_str("https://api.example.com/v1").unwrap(),
            HttpOrigin::from_str("https://api.example.com/openai/v1").unwrap()
        );

        for invalid in [
            "ftp://example.com",
            "https://user@example.com",
            "https://user:pass@example.com",
        ] {
            assert!(HttpOrigin::from_str(invalid).is_err(), "accepted {invalid}");
        }
    }
}
