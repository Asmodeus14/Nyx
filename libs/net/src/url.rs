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

#[derive(Debug, Clone, PartialEq)]
pub enum ParseError {
    /// Malformed in some way — the message names which.
    Bad(&'static str),
    /// Well-formed, and not something a fetch can act on: `mailto:`, `tel:`, `data:`, `ftp:`.
    ///
    /// ★ Separate from `Bad` because it is not a mistake, it is a link a page legitimately
    /// contains, and the caller wants to say "that is an email address" rather than "bad URL".
    /// Before this existed, `join` did not recognise a scheme without `//` at all and turned
    /// `mailto:a@b` into the *path* `/mailto:a@b`, which was then requested from whatever host the
    /// reader happened to be on.
    UnsupportedScheme(String),
}

impl fmt::Display for ParseError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            ParseError::Bad(m) => write!(f, "bad URL: {m}"),
            ParseError::UnsupportedScheme(s) => {
                write!(f, "{s}: links are not pages (only http and https can be fetched)")
            }
        }
    }
}

/// The longest URL accepted. Well past anything real — the cap exists so a hostile page cannot
/// hand the parser an unbounded string, not to limit legitimate addresses.
pub const MAX_URL_LEN: usize = 8192;

/// The scheme prefix of `s`, if it has one: the text before the first `:`, when that text is a
/// legal scheme (ALPHA *( ALPHA / DIGIT / "+" / "-" / "." ), RFC 3986 §3.1).
///
/// Used to tell `mailto:a@b` from the relative path `a/b`. A bare `:` inside a path segment is
/// legal and must NOT be mistaken for a scheme, which is why the character rules are enforced
/// rather than just searching for a colon.
fn scheme_prefix(s: &str) -> Option<&str> {
    let colon = s.find(':')?;
    let head = &s[..colon];
    let mut chars = head.chars();
    let first = chars.next()?;
    if !first.is_ascii_alphabetic() {
        return None;
    }
    if chars.all(|c| c.is_ascii_alphanumeric() || matches!(c, '+' | '-' | '.')) {
        Some(head)
    } else {
        None
    }
}

impl Url {
    pub fn parse(input: &str) -> Result<Url, ParseError> {
        let input = input.trim();
        if input.len() > MAX_URL_LEN {
            return Err(ParseError::Bad("URL is absurdly long"));
        }

        // Scheme. A bare "example.com" is treated as http:// — that is what a user typing into an
        // address bar means, and refusing it would only be pedantry.
        //
        // Anything else with a scheme is reported by NAME. `mailto:`, `tel:` and `data:` have no
        // `//`, so testing for `://` misses them entirely and they fall through to be treated as a
        // hostname or a path — which is how `mailto:a@b` became a request for `/mailto:a@b`.
        let (scheme, rest) = if let Some(r) = strip_prefix_ci(input, "https://") {
            (Scheme::Https, r)
        } else if let Some(r) = strip_prefix_ci(input, "http://") {
            (Scheme::Http, r)
        } else {
            match scheme_prefix(input) {
                Some(s) => return Err(ParseError::UnsupportedScheme(s.to_ascii_lowercase())),
                None => (Scheme::Http, input),
            }
        };

        // The fragment is client-side only and must never be sent to the server.
        let rest = rest.split('#').next().unwrap_or("");

        // Authority ends at the first '/', '?' — whichever comes first.
        let auth_end = rest.find(['/', '?']).unwrap_or(rest.len());
        let authority = &rest[..auth_end];
        let path_part = &rest[auth_end..];

        if authority.is_empty() {
            return Err(ParseError::Bad("empty host"));
        }

        // Userinfo (`user:pass@host`) belongs to the authority and is not part of the host. Split
        // at the LAST '@' — a password may legally contain one. Dropped rather than kept: nothing
        // in this client does HTTP authentication, and carrying a credential we never use would be
        // a credential we might one day leak.
        let authority = match authority.rfind('@') {
            Some(at) => &authority[at + 1..],
            None => authority,
        };
        if authority.is_empty() {
            return Err(ParseError::Bad("empty host"));
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
                None => return Err(ParseError::Bad("unterminated IPv6 literal")),
            }
        } else {
            match authority.rsplit_once(':') {
                Some((h, p)) => (h.to_string(), parse_port(Some(p), scheme)?),
                None => (authority.to_string(), scheme.default_port()),
            }
        };

        if host.is_empty() {
            return Err(ParseError::Bad("empty host"));
        }
        // Hostnames are case-insensitive (RFC 4343). Normalising here means `Example.COM` and
        // `example.com` are one entry in the DNS cache instead of two, and one `Host:` header
        // instead of two spellings of it.
        let host = host.to_ascii_lowercase();

        let path = if path_part.is_empty() {
            String::from("/")
        } else if path_part.starts_with('?') {
            format!("/{path_part}")
        } else {
            normalize_path(path_part)
        };

        Ok(Url { scheme, host, port, path: encode_path(&path) })
    }

    /// Resolve a `Location` header against this URL. Handles the three forms servers actually send:
    /// absolute (`https://…`), scheme-relative (`//host/…`), and path-relative (`/x` or `x`).
    pub fn join(&self, location: &str) -> Result<Url, ParseError> {
        let location = location.trim();
        if location.is_empty() {
            return Err(ParseError::Bad("empty Location"));
        }

        // An absolute URL of any scheme. Checked with the same scheme rules `parse` uses, so a
        // `mailto:` in an href is reported as an unfetchable link rather than silently becoming a
        // path on the current host — which is what happened when this only tested for `://`.
        //
        // Guarded on `//` for the scheme-relative form: `//host/x` has no scheme, and `scheme_prefix`
        // would not match it anyway, but the ordering makes the intent explicit.
        if !location.starts_with("//") {
            if let Some(sch) = scheme_prefix(location) {
                return if sch.eq_ignore_ascii_case("http") || sch.eq_ignore_ascii_case("https") {
                    Url::parse(location)
                } else {
                    Err(ParseError::UnsupportedScheme(sch.to_ascii_lowercase()))
                };
            }
        }

        if let Some(rest) = location.strip_prefix("//") {
            let scheme = if self.scheme == Scheme::Https { "https" } else { "http" };
            return Url::parse(&format!("{scheme}://{rest}"));
        }

        let path = if location.starts_with('/') {
            normalize_path(location.split('#').next().unwrap_or("/"))
        } else {
            // Relative to the current directory, i.e. everything up to and including the last '/'.
            // The base's own query is dropped — `?a=1` on the page does not carry to its links.
            let base_path = self.path.split('?').next().unwrap_or("/");
            let base = match base_path.rfind('/') {
                Some(i) => &base_path[..=i],
                None => "/",
            };
            normalize_path(&format!("{base}{}", location.split('#').next().unwrap_or("")))
        };

        Ok(Url { scheme: self.scheme, host: self.host.clone(), port: self.port, path })
    }
}

/// Resolve `.` and `..` segments, per RFC 3986 §5.2.4.
///
/// Without this, `join` is string concatenation with extra steps: `/a/b/` + `../c` produces
/// `/a/b/../c`, which is a path the server has never heard of. Most will 404 it; some normalise it
/// themselves and serve the right thing, which is worse, because it means the bug only shows up on
/// *some* sites and looks like those sites being broken.
///
/// A `..` that would climb above the root is discarded rather than escaping — the spec's rule, and
/// also the one that stops `/../../../etc/passwd` from being expressible on the request line.
fn normalize_path(path: &str) -> String {
    // Only the path is normalised. A `..` inside a query string is data, not a path segment.
    let (p, query) = match path.find('?') {
        Some(i) => (&path[..i], &path[i..]),
        None => (path, ""),
    };

    // A path ending in a separator (or in a `.`/`..` that resolves to one) names a directory, and
    // that distinction survives normalisation — `/a/b/` and `/a/b` are different base URLs for the
    // *next* relative link.
    let directory = p.ends_with('/') || p.ends_with("/.") || p.ends_with("/..") || p == "." || p == "..";

    let mut out: Vec<&str> = Vec::new();
    for seg in p.split('/') {
        match seg {
            "" | "." => {}
            ".." => {
                out.pop();
            }
            s => out.push(s),
        }
    }

    let mut s = String::with_capacity(p.len() + 1);
    s.push('/');
    s.push_str(&out.join("/"));
    if directory && !s.ends_with('/') {
        s.push('/');
    }
    s.push_str(query);
    s
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
        // Port 0 parses fine and is not a port: it means "any" to bind() and is meaningless to
        // connect(), so accepting it turns a typo into a connection attempt that cannot succeed.
        Some(p) => match p.parse::<u16>() {
            Ok(0) => Err(ParseError::Bad("port 0 is not a port")),
            Ok(n) => Ok(n),
            Err(_) => Err(ParseError::Bad("bad port")),
        },
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

    // -- Phase 8: every URL a real page can contain ---------------------------------------------

    #[test]
    fn a_link_that_is_not_a_page_is_named_rather_than_fetched() {
        // The failure this guards: `join` tested only for "://", so `mailto:` (which has no
        // slashes) fell through to the RELATIVE-PATH branch and became `/mailto:a@b` -- a request
        // sent to whatever host the reader was on. `libs/htmltext` numbers these links, so
        // `open <n>` hit it routinely.
        let base = Url::parse("https://a.com/page").unwrap();
        for href in ["mailto:someone@example.com", "tel:+15551234", "data:text/plain,hi", "ftp://x/"] {
            match base.join(href) {
                Err(ParseError::UnsupportedScheme(_)) => {}
                other => panic!("{href:?} should be reported as unsupported, got {other:?}"),
            }
        }
        // And the same through `parse`, for a URL typed directly.
        assert!(matches!(
            Url::parse("mailto:a@b"),
            Err(ParseError::UnsupportedScheme(ref s)) if s == "mailto"
        ));
    }

    #[test]
    fn a_colon_in_the_first_segment_is_a_scheme_and_dot_slash_escapes_it() {
        // RFC 3986 4.2: a relative reference whose FIRST segment contains a colon is ambiguous with
        // a scheme, and the spec's remedy is to write `./`. Browsers resolve the ambiguity the same
        // way we do -- `odd:name` is a scheme -- so this is behaviour to pin down, not a bug.
        let base = Url::parse("https://a.com/dir/").unwrap();
        assert!(matches!(base.join("odd:name"), Err(ParseError::UnsupportedScheme(_))));
        assert_eq!(base.join("./odd:name").unwrap().path, "/dir/odd:name");
        // A colon in a LATER segment is unambiguous and stays a path.
        assert_eq!(base.join("a/2:1").unwrap().path, "/dir/a/2:1");
        // A rooted path is never a scheme either.
        assert_eq!(base.join("/x:y").unwrap().path, "/x:y");
    }

    #[test]
    fn userinfo_is_stripped_rather_than_breaking_the_parse() {
        // Previously `user:pass@host` split at the last ':' and produced host="user",
        // port="pass@host" -> "bad port". A legal URL that simply failed.
        let u = Url::parse("https://user:pass@example.com/x").unwrap();
        assert_eq!(u.host, "example.com");
        assert_eq!(u.port, 443);
        assert_eq!(u.path, "/x");
        assert_eq!(Url::parse("http://user@example.com:8080/").unwrap().port, 8080);
        // The '@' inside a password must not end the userinfo early.
        assert_eq!(Url::parse("http://u:p@ss@example.com/").unwrap().host, "example.com");
    }

    #[test]
    fn the_host_is_case_folded() {
        // Hostnames are case-insensitive (RFC 4343). Folding keeps the DNS cache from holding the
        // same host twice under two spellings.
        assert_eq!(Url::parse("HTTPS://Example.COM/X").unwrap().host, "example.com");
        // ...but the PATH is case-sensitive and must not be touched.
        assert_eq!(Url::parse("https://a.com/CaseSensitive").unwrap().path, "/CaseSensitive");
    }

    #[test]
    fn the_request_target_is_percent_encoded() {
        // The path is interpolated into `GET {path} HTTP/1.1`, which is SPACE-delimited -- so an
        // unencoded space produces a malformed request line, and a CR/LF would let a crafted link
        // inject headers.
        assert_eq!(Url::parse("http://a.com/a b").unwrap().path, "/a%20b");
        assert_eq!(Url::parse("http://a.com/a\r\nX: y").unwrap().path, "/a%0D%0AX:%20y");
        // Already-encoded input is left alone -- re-encoding %20 into %2520 asks for another file.
        assert_eq!(Url::parse("http://a.com/a%20b").unwrap().path, "/a%20b");
        // Query structure and sub-delims survive untouched.
        assert_eq!(Url::parse("http://a.com/s?q=a+b&r=1,2;3").unwrap().path, "/s?q=a+b&r=1,2;3");
        // Non-ASCII becomes UTF-8 percent triplets rather than raw bytes on the wire.
        assert_eq!(Url::parse("http://a.com/caf\u{e9}").unwrap().path, "/caf%C3%A9");
    }

    #[test]
    fn port_zero_is_refused() {
        // Port 0 parses as a u16 and is not a port: connect() cannot use it, so accepting it turns
        // a typo into a connection attempt that can never succeed.
        assert!(Url::parse("http://a.com:0/").is_err());
        assert!(Url::parse("http://a.com:99999/").is_err());
        assert_eq!(Url::parse("http://a.com:8443/x").unwrap().port, 8443);
    }

    #[test]
    fn an_absurdly_long_url_is_refused_rather_than_parsed() {
        let long = format!("http://a.com/{}", "x".repeat(MAX_URL_LEN));
        assert!(Url::parse(&long).is_err());
    }

    #[test]
    fn the_phase_8_table_round_trips() {
        let cases = [
            ("https://example.com", "example.com", 443u16, "/"),
            ("https://example.com/", "example.com", 443, "/"),
            ("https://example.com/path", "example.com", 443, "/path"),
            ("https://example.com/path?a=b", "example.com", 443, "/path?a=b"),
            ("https://example.com:8443/path", "example.com", 8443, "/path"),
            ("http://example.com", "example.com", 80, "/"),
        ];
        for (input, host, port, path) in cases {
            let u = Url::parse(input).unwrap_or_else(|e| panic!("{input}: {e}"));
            assert_eq!((u.host.as_str(), u.port, u.path.as_str()), (host, port, path), "{input}");
        }
    }

    #[test]
    fn dot_dot_climbs_instead_of_being_concatenated() {
        // The failure this guards: `join` used to produce `/x/y/../c`, a path no server has heard
        // of. Some 404 it and some normalise it themselves, so the bug looks like a site problem.
        let base = Url::parse("https://a.com/x/y/").unwrap();
        assert_eq!(base.join("../c").unwrap().path, "/x/c");
        assert_eq!(base.join("../../c").unwrap().path, "/c");
        assert_eq!(base.join("./c").unwrap().path, "/x/y/c");
    }

    #[test]
    fn dot_dot_cannot_climb_above_the_root() {
        let base = Url::parse("https://a.com/x/").unwrap();
        assert_eq!(base.join("../../../../etc/passwd").unwrap().path, "/etc/passwd");
    }

    #[test]
    fn a_normalised_directory_stays_a_directory() {
        // `/a/b/` and `/a/b` are different bases for the NEXT relative link, so the trailing
        // separator has to survive normalisation.
        let base = Url::parse("https://a.com/x/y/z").unwrap();
        assert_eq!(base.join("..").unwrap().path, "/x/");
        assert_eq!(Url::parse("https://a.com/a/./b/").unwrap().path, "/a/b/");
    }

    #[test]
    fn a_query_string_is_not_a_path_and_is_not_normalised() {
        let u = Url::parse("https://a.com/x/../y?next=/a/../b").unwrap();
        assert_eq!(u.path, "/y?next=/a/../b");
    }

    #[test]
    fn the_bases_own_query_does_not_carry_to_its_links() {
        let base = Url::parse("https://a.com/dir/page?session=1").unwrap();
        assert_eq!(base.join("other").unwrap().path, "/dir/other");
    }
}

/// Percent-encode the characters that cannot appear in a request line.
///
/// The path is interpolated straight into `GET {path} HTTP/1.1`, where the target is delimited by
/// spaces — so a single unencoded space produces `GET /a b HTTP/1.1`, which a server reads as a
/// malformed request or as a request for `/a` with a bogus version. Control characters are worse:
/// a CR or LF would end the request line and let a crafted link inject headers.
///
/// Deliberately conservative. Anything already percent-encoded is left alone (`%` passes through),
/// because re-encoding `%20` into `%2520` would change which resource is being asked for. The
/// sub-delimiters and the reserved characters that carry meaning in a path or query (`/?:@&=+$,;`
/// and friends) also pass through unchanged, for the same reason.
fn encode_path(path: &str) -> String {
    fn safe(b: u8) -> bool {
        b.is_ascii_alphanumeric()
            || matches!(
                b,
                b'-' | b'.' | b'_' | b'~'          // unreserved (RFC 3986 §2.3)
                | b'/' | b'?' | b'#' | b'[' | b']' | b'@'   // gen-delims that structure the target
                | b'!' | b'$' | b'&' | b'\'' | b'(' | b')'
                | b'*' | b'+' | b',' | b';' | b'=' | b':'   // sub-delims
                | b'%'                                       // already-encoded triplets
            )
    }

    if path.bytes().all(safe) {
        return path.to_string();
    }
    let mut out = String::with_capacity(path.len() + 8);
    for b in path.bytes() {
        if safe(b) {
            out.push(b as char);
        } else {
            out.push('%');
            out.push(char::from_digit((b >> 4) as u32, 16).unwrap_or('0').to_ascii_uppercase());
            out.push(char::from_digit((b & 0xF) as u32, 16).unwrap_or('0').to_ascii_uppercase());
        }
    }
    out
}
