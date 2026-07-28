//! The small amount of URL handling an HTTP client actually needs.
//!
//! Deliberately not a general URL crate: no userinfo, no IDN, no percent-encoding normalisation.
//! Those matter for a browser's address bar and can be added when there is one; a fetch client only
//! has to split a string into "where do I connect" and "what do I put on the request line".

use core::fmt;

#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Scheme {
    Http,
    Https,
}

impl Scheme {
    pub fn default_port(self) -> u16 {
        match self {
            Scheme::Http => 80,
            Scheme::Https => 443,
        }
    }
}

#[derive(Clone, Debug)]
pub struct Url {
    pub scheme: Scheme,
    pub host: String,
    pub port: u16,
    /// Path plus query, exactly as it goes on the request line. Never empty — "/" at minimum.
    pub path: String,
}

#[derive(Debug)]
pub struct ParseError(&'static str);

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "bad URL: {}", self.0)
    }
}

impl Url {
    pub fn parse(input: &str) -> Result<Url, ParseError> {
        let input = input.trim();

        // Scheme. A bare "example.com" is treated as http:// — that is what a user typing into an
        // address bar means, and refusing it would only be pedantry.
        let (scheme, rest) = if let Some(r) = strip_prefix_ci(input, "https://") {
            (Scheme::Https, r)
        } else if let Some(r) = strip_prefix_ci(input, "http://") {
            (Scheme::Http, r)
        } else if input.contains("://") {
            return Err(ParseError("unsupported scheme (only http and https)"));
        } else {
            (Scheme::Http, input)
        };

        // The fragment is client-side only and must never be sent to the server.
        let rest = rest.split('#').next().unwrap_or("");

        // Authority ends at the first '/', '?' — whichever comes first.
        let auth_end = rest.find(['/', '?']).unwrap_or(rest.len());
        let authority = &rest[..auth_end];
        let path_part = &rest[auth_end..];

        if authority.is_empty() {
            return Err(ParseError("empty host"));
        }

        // Split host:port from the RIGHT, and only when the colon is not inside a bracketed IPv6
        // literal. Nyx has no IPv6 stack, so a literal will fail later at connect — but it should
        // fail there with a clear message, not get mis-parsed into a nonsense host here.
        let (host, port) = if authority.starts_with('[') {
            match authority.find(']') {
                Some(close) => {
                    let host = &authority[..=close];
                    let after = &authority[close + 1..];
                    let port = parse_port(after.strip_prefix(':'), scheme)?;
                    (host.to_string(), port)
                }
                None => return Err(ParseError("unterminated IPv6 literal")),
            }
        } else {
            match authority.rsplit_once(':') {
                Some((h, p)) => (h.to_string(), parse_port(Some(p), scheme)?),
                None => (authority.to_string(), scheme.default_port()),
            }
        };

        if host.is_empty() {
            return Err(ParseError("empty host"));
        }

        let path = if path_part.is_empty() {
            String::from("/")
        } else if path_part.starts_with('?') {
            format!("/{path_part}")
        } else {
            path_part.to_string()
        };

        Ok(Url { scheme, host, port, path })
    }

    /// Resolve a `Location` header against this URL. Handles the three forms servers actually send:
    /// absolute (`https://…`), scheme-relative (`//host/…`), and path-relative (`/x` or `x`).
    pub fn join(&self, location: &str) -> Result<Url, ParseError> {
        let location = location.trim();
        if location.is_empty() {
            return Err(ParseError("empty Location"));
        }

        if location.contains("://") {
            return Url::parse(location);
        }

        if let Some(rest) = location.strip_prefix("//") {
            let scheme = if self.scheme == Scheme::Https { "https" } else { "http" };
            return Url::parse(&format!("{scheme}://{rest}"));
        }

        let path = if location.starts_with('/') {
            location.split('#').next().unwrap_or("/").to_string()
        } else {
            // Relative to the current directory, i.e. everything up to and including the last '/'.
            let base = match self.path.rfind('/') {
                Some(i) => &self.path[..=i],
                None => "/",
            };
            format!("{base}{}", location.split('#').next().unwrap_or(""))
        };

        Ok(Url { scheme: self.scheme, host: self.host.clone(), port: self.port, path })
    }
}

impl fmt::Display for Url {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let scheme = match self.scheme {
            Scheme::Http => "http",
            Scheme::Https => "https",
        };
        if self.port == self.scheme.default_port() {
            write!(f, "{scheme}://{}{}", self.host, self.path)
        } else {
            write!(f, "{scheme}://{}:{}{}", self.host, self.port, self.path)
        }
    }
}

fn parse_port(text: Option<&str>, scheme: Scheme) -> Result<u16, ParseError> {
    match text {
        None => Ok(scheme.default_port()),
        // "host:" is legal and means the default port.
        Some("") => Ok(scheme.default_port()),
        Some(p) => p.parse::<u16>().map_err(|_| ParseError("bad port")),
    }
}

/// Schemes are case-insensitive per RFC 3986, and "HTTP://" does appear in the wild.
fn strip_prefix_ci<'a>(s: &'a str, prefix: &str) -> Option<&'a str> {
    if s.len() >= prefix.len() && s[..prefix.len()].eq_ignore_ascii_case(prefix) {
        Some(&s[prefix.len()..])
    } else {
        None
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn bare_host_defaults_to_http_and_root() {
        let u = Url::parse("example.com").unwrap();
        assert_eq!(u.scheme, Scheme::Http);
        assert_eq!(u.host, "example.com");
        assert_eq!(u.port, 80);
        assert_eq!(u.path, "/");
    }

    #[test]
    fn https_with_port_and_query() {
        let u = Url::parse("https://example.com:8443/a/b?x=1&y=2").unwrap();
        assert_eq!(u.scheme, Scheme::Https);
        assert_eq!(u.port, 8443);
        assert_eq!(u.path, "/a/b?x=1&y=2");
    }

    #[test]
    fn fragment_is_never_sent() {
        let u = Url::parse("http://example.com/page#section").unwrap();
        assert_eq!(u.path, "/page");
    }

    #[test]
    fn query_only_gets_a_root_path() {
        let u = Url::parse("http://example.com?q=1").unwrap();
        assert_eq!(u.path, "/?q=1");
    }

    #[test]
    fn scheme_is_case_insensitive() {
        assert_eq!(Url::parse("HTTPS://example.com").unwrap().scheme, Scheme::Https);
    }

    #[test]
    fn join_absolute_replaces_everything() {
        let base = Url::parse("https://a.com/x/y").unwrap();
        let u = base.join("http://b.com/z").unwrap();
        assert_eq!(u.host, "b.com");
        assert_eq!(u.scheme, Scheme::Http);
        assert_eq!(u.path, "/z");
    }

    #[test]
    fn join_scheme_relative_keeps_scheme() {
        let base = Url::parse("https://a.com/x").unwrap();
        let u = base.join("//b.com/z").unwrap();
        assert_eq!(u.scheme, Scheme::Https);
        assert_eq!(u.host, "b.com");
    }

    #[test]
    fn join_absolute_path_replaces_path_only() {
        let base = Url::parse("https://a.com/x/y?q=1").unwrap();
        let u = base.join("/z").unwrap();
        assert_eq!(u.host, "a.com");
        assert_eq!(u.path, "/z");
    }

    #[test]
    fn join_relative_path_is_directory_relative() {
        let base = Url::parse("https://a.com/x/y").unwrap();
        assert_eq!(base.join("z").unwrap().path, "/x/z");
    }

    #[test]
    fn display_omits_the_default_port() {
        assert_eq!(Url::parse("https://a.com/x").unwrap().to_string(), "https://a.com/x");
        assert_eq!(Url::parse("https://a.com:9/x").unwrap().to_string(), "https://a.com:9/x");
    }

    #[test]
    fn unsupported_scheme_is_rejected() {
        assert!(Url::parse("ftp://a.com").is_err());
    }
}
