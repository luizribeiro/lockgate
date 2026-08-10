//! Validated outbound HTTP origins retained by capability grants.

use http::Uri;
use url::Url;

#[derive(Clone, Debug, PartialEq, Eq)]
pub(crate) struct HttpOrigin {
    scheme: &'static str,
    host: String,
    port: u16,
}

impl HttpOrigin {
    pub(crate) fn parse(value: &str) -> Result<Self, ()> {
        let url = Url::parse(value).map_err(|_| ())?;
        let scheme = match url.scheme() {
            "http" => "http",
            "https" => "https",
            _ => return Err(()),
        };
        if !url.username().is_empty()
            || url.password().is_some()
            || url.path() != "/"
            || url.query().is_some()
            || url.fragment().is_some()
        {
            return Err(());
        }
        let host = url.host_str().ok_or(())?.to_ascii_lowercase();
        let port = url.port_or_known_default().ok_or(())?;
        Ok(Self { scheme, host, port })
    }

    pub(crate) fn matches(&self, uri: &Uri) -> bool {
        let Some(scheme) = uri.scheme_str() else {
            return false;
        };
        let Some(authority) = uri.authority() else {
            return false;
        };
        let port = authority.port_u16().unwrap_or_else(|| default_port(scheme));
        self.scheme == scheme
            && self.host.eq_ignore_ascii_case(authority.host())
            && self.port == port
    }
}

fn default_port(scheme: &str) -> u16 {
    if scheme == "https" { 443 } else { 80 }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn matches_only_the_configured_origin() {
        let origin = HttpOrigin::parse("https://EXAMPLE.com").unwrap();

        assert!(origin.matches(&"https://example.com/path?q=1".parse().unwrap()));
        assert!(origin.matches(&"https://example.com:443/".parse().unwrap()));
        assert!(!origin.matches(&"http://example.com/".parse().unwrap()));
        assert!(!origin.matches(&"https://example.com:8443/".parse().unwrap()));
        assert!(!origin.matches(&"https://sub.example.com/".parse().unwrap()));

        let ipv6 = HttpOrigin::parse("https://[::1]:8443").unwrap();
        assert!(ipv6.matches(&"https://[::1]:8443/path".parse().unwrap()));
    }

    #[test]
    fn accepts_only_http_origins() {
        for value in [
            "example.com",
            "ftp://example.com",
            "https://example.com/path",
            "https://example.com?query",
            "https://example.com/#fragment",
        ] {
            assert!(HttpOrigin::parse(value).is_err(), "accepted `{value}`");
        }

        assert!(HttpOrigin::parse("http://127.0.0.1:8080").is_ok());
        assert!(HttpOrigin::parse("https://[::1]:8443").is_ok());
    }
}
