//! HTTP/1.1 over plain TCP or TLS 1.3.
//!
//! Scope is "fetch a document", which is what a browser's network layer has to do first. One
//! request per connection (`Connection: close`) — no keep-alive, no pipelining, no HTTP/2. Those are
//! throughput optimisations; correctness first.
//!
//! `Accept-Encoding: identity` is deliberate: it keeps gzip/brotli decoders out of the dependency
//! graph for now. Every server must honour it (RFC 9110 §12.5.3).

use crate::url::{Scheme, Url};
use std::io::{Read, Write};
use std::net::TcpStream;
use std::sync::{Arc, OnceLock};
use std::time::{Duration, Instant};

/// Deadline for any single blocking socket operation.
///
/// Not optional. Nyx's blocking read/write loops live in the kernel and only exit on data or on the
/// link dying, so without this a stalled peer (or a TLS handshake that stops mid-flight) parks the
/// calling thread forever — and since the terminal runs `fetch` synchronously, "forever" means a
/// frozen UI with nothing on screen to say why.
const IO_TIMEOUT: Duration = Duration::from_secs(20);

/// Phase tracing to stderr, which on Nyx is the serial log.
///
/// This is the only debugger available on the target: the GUI is blocked for the whole request, so
/// anything that goes wrong is invisible until it finishes. Each line is timestamped from the start
/// of the request, which turns "it hung" into "it hung *there*, after N ms".
macro_rules! trace {
    ($start:expr, $($arg:tt)*) => {
        eprintln!("[net +{:>6}ms] {}", $start.elapsed().as_millis(), format_args!($($arg)*))
    };
}

/// A response body larger than this is refused rather than allowed to exhaust the userspace heap.
/// Nyx processes do not get an OOM killer; an unbounded read would just wedge the machine.
pub(crate) const MAX_BODY: usize = 8 * 1024 * 1024;
/// Headers past this point are a malformed or hostile server, not a real response.
const MAX_HEADERS: usize = 64 * 1024;
/// Redirect chains longer than this are a loop.
const MAX_REDIRECTS: usize = 8;

#[derive(Debug)]
pub enum Error {
    Url(crate::url::ParseError),
    Io(std::io::Error),
    Tls(rustls::Error),
    /// The server said something that is not HTTP.
    Protocol(String),
    TooLarge(&'static str),
    TooManyRedirects,
}

impl std::fmt::Display for Error {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Error::Url(e) => write!(f, "{e}"),
            Error::Io(e) => write!(f, "{e}"),
            Error::Tls(e) => write!(f, "TLS error: {e}"),
            Error::Protocol(s) => write!(f, "bad HTTP response: {s}"),
            Error::TooLarge(what) => write!(f, "{what} exceeded the size limit"),
            Error::TooManyRedirects => write!(f, "too many redirects"),
        }
    }
}

impl From<std::io::Error> for Error {
    fn from(e: std::io::Error) -> Self {
        Error::Io(e)
    }
}
impl From<rustls::Error> for Error {
    fn from(e: rustls::Error) -> Self {
        Error::Tls(e)
    }
}
impl From<crate::url::ParseError> for Error {
    fn from(e: crate::url::ParseError) -> Self {
        Error::Url(e)
    }
}

pub struct Response {
    pub status: u16,
    pub headers: Vec<(String, String)>,
    pub body: Vec<u8>,
    /// Where the body actually came from, after any redirects. Relative links must resolve against
    /// this, not against what the caller asked for.
    pub url: Url,
}

impl Response {
    /// Header lookup is case-insensitive (RFC 9110 §5.1) — servers are inconsistent about casing.
    pub fn header(&self, name: &str) -> Option<&str> {
        self.headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    }

    pub fn content_type(&self) -> Option<&str> {
        self.header("content-type")
    }

    /// Body as text. Lossy on purpose: a browser renders what it can rather than refusing a page
    /// over one bad byte. Charset sniffing beyond UTF-8 is a later problem.
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.body).into_owned()
    }
}

/// The rustls client config, built once. Loading 100+ root certificates and building the provider
/// on every request would be pure waste on a machine this size.
fn tls_config() -> Arc<rustls::ClientConfig> {
    static CONFIG: OnceLock<Arc<rustls::ClientConfig>> = OnceLock::new();
    CONFIG
        .get_or_init(|| {
            let mut roots = rustls::RootCertStore::empty();
            roots.extend(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
            ROOT_COUNT.store(roots.len(), std::sync::atomic::Ordering::Relaxed);
            let cfg = rustls::ClientConfig::builder_with_provider(Arc::new(
                rustls_rustcrypto::provider(),
            ))
            .with_safe_default_protocol_versions()
            .expect("rustls default protocol versions")
            .with_root_certificates(roots)
            .with_no_client_auth();
            Arc::new(cfg)
        })
        .clone()
}

/// Plain or encrypted, behind one Read+Write so the HTTP code never branches on it.
pub(crate) enum Transport {
    Plain(TcpStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, TcpStream>>),
}

impl Read for Transport {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        match self {
            Transport::Plain(s) => s.read(buf),
            Transport::Tls(s) => s.read(buf),
        }
    }
}

impl Write for Transport {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        match self {
            Transport::Plain(s) => s.write(buf),
            Transport::Tls(s) => s.write(buf),
        }
    }
    fn flush(&mut self) -> std::io::Result<()> {
        match self {
            Transport::Plain(s) => s.flush(),
            Transport::Tls(s) => s.flush(),
        }
    }
}

impl Transport {
    /// `addr` is pre-resolved by the caller. Passing it in rather than handing
    /// `(host, port)` to `TcpStream::connect` keeps DNS and the TCP handshake as two separately
    /// observable steps — they fail for unrelated reasons and each can stall on its own.
    pub(crate) fn connect(
        url: &Url,
        addr: std::net::SocketAddr,
        start: &Instant,
    ) -> Result<Transport, Error> {
        trace!(start, "connecting to {addr} for {}:{}", url.host, url.port);
        let sock = TcpStream::connect(addr)?;
        trace!(start, "tcp connected");

        // Arm the deadline before any traffic. Doing it after the handshake would leave exactly the
        // window that hangs — the handshake is the part most likely to stall.
        sock.set_read_timeout(Some(IO_TIMEOUT))?;
        sock.set_write_timeout(Some(IO_TIMEOUT))?;

        match url.scheme {
            Scheme::Http => Ok(Transport::Plain(sock)),
            Scheme::Https => {
                let name = url
                    .host
                    .clone()
                    .try_into()
                    .map_err(|_| Error::Protocol(format!("{} is not a valid DNS name", url.host)))?;
                // Building the config is where the root store and crypto provider get set up on
                // first use; on a cold cache that cost lands here, not in the handshake.
                let config = tls_config();
                trace!(start, "tls config ready ({} roots)", config.root_store_size());
                let conn = rustls::ClientConnection::new(config, name)?;
                trace!(start, "tls client created; handshake runs on first read/write");
                Ok(Transport::Tls(Box::new(rustls::StreamOwned::new(conn, sock))))
            }
        }
    }

    /// Re-arm both socket deadlines. The stepped fetch uses a much shorter one than `get`, because
    /// there a timeout means "come back next frame" rather than "give up".
    pub(crate) fn set_timeout(&mut self, d: Duration) -> Result<(), Error> {
        let sock = match self {
            Transport::Plain(s) => s,
            Transport::Tls(s) => &mut s.sock,
        };
        sock.set_read_timeout(Some(d))?;
        sock.set_write_timeout(Some(d))?;
        Ok(())
    }

    /// Hand the request over without waiting for it to reach the wire.
    ///
    /// For TLS this only fills rustls's own outgoing buffer — no I/O and so nothing to time out. The
    /// handshake and the flush both happen inside the first `read`, which is exactly where the
    /// stepped fetch can afford to be interrupted and resume. Writing here instead would put a
    /// blocking handshake in a path that has no way to report progress.
    pub(crate) fn queue_request(&mut self, req: &[u8]) -> Result<(), Error> {
        match self {
            // A request this small always fits in the socket's send buffer, so this does not block.
            Transport::Plain(s) => {
                s.write_all(req)?;
                s.flush()?;
            }
            Transport::Tls(s) => {
                s.conn.writer().write_all(req)?;
            }
        }
        Ok(())
    }
}

/// The request bytes for a GET. Shared so the blocking and stepped paths cannot drift apart.
pub(crate) fn request_line(url: &Url) -> Vec<u8> {
    // Host must carry the port when it is non-default, or name-based virtual hosts answer wrong.
    let host_header = if url.port == url.scheme.default_port() {
        url.host.clone()
    } else {
        format!("{}:{}", url.host, url.port)
    };
    format!(
        "GET {} HTTP/1.1\r\n\
         Host: {host_header}\r\n\
         User-Agent: Nyx/0.1\r\n\
         Accept: */*\r\n\
         Accept-Encoding: identity\r\n\
         Connection: close\r\n\r\n",
        url.path
    )
    .into_bytes()
}

/// rustls does not expose the root count once the config is built, and the number is genuinely
/// useful in the trace — an empty root store fails every certificate for a reason that reads like a
/// network problem. Recorded when the config is created.
trait RootCount {
    fn root_store_size(&self) -> usize;
}
impl RootCount for Arc<rustls::ClientConfig> {
    fn root_store_size(&self) -> usize {
        ROOT_COUNT.load(std::sync::atomic::Ordering::Relaxed)
    }
}
static ROOT_COUNT: std::sync::atomic::AtomicUsize = std::sync::atomic::AtomicUsize::new(0);

/// GET a URL, following redirects.
pub fn get(url: &str) -> Result<Response, Error> {
    let mut current = Url::parse(url)?;
    for _ in 0..MAX_REDIRECTS {
        let resp = get_once(&current)?;
        match resp.status {
            301 | 302 | 303 | 307 | 308 => {
                // Take the redirect only if it actually points somewhere; a 3xx with no Location is
                // the server's bug and the response body is more useful to the caller than an error.
                let Some(location) = resp.header("location") else {
                    return Ok(resp);
                };
                current = current.join(location)?;
            }
            _ => return Ok(resp),
        }
    }
    Err(Error::TooManyRedirects)
}

/// One request, no redirect handling.
pub fn get_once(url: &Url) -> Result<Response, Error> {
    let start = Instant::now();
    trace!(&start, "GET {url}");
    let addr = crate::fetch::resolve_host(url)?;
    let mut transport = Transport::connect(url, addr, &start)?;

    // For HTTPS this is where the handshake actually happens — rustls defers it to the first I/O —
    // so a stall here is a TLS problem, not a request-sending problem.
    transport.write_all(&request_line(url))?;
    transport.flush()?;
    trace!(&start, "request sent (handshake done for https)");

    let (head, leftover) = read_head(&mut transport)?;
    let (status, headers) = parse_head(&head)?;
    trace!(&start, "headers: status {status}, {} fields", headers.len());

    let body = read_body(&mut transport, &headers, leftover)?;
    trace!(&start, "body complete: {} bytes", body.len());

    Ok(Response { status, headers, body, url: url.clone() })
}

/// Read until the CRLFCRLF that ends the header block. Returns the header bytes and whatever body
/// bytes arrived in the same read — those must not be dropped, they are the start of the body.
fn read_head(transport: &mut Transport) -> Result<(Vec<u8>, Vec<u8>), Error> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    // Where to resume scanning. The terminator can straddle two reads, so back up 3 bytes.
    let mut scanned = 0usize;

    loop {
        let n = match transport.read(&mut chunk) {
            Ok(0) => return Err(Error::Protocol("connection closed before headers".into())),
            Ok(n) => n,
            Err(e) if is_clean_eof(&e) => {
                return Err(Error::Protocol("connection closed before headers".into()));
            }
            Err(e) => return Err(e.into()),
        };
        buf.extend_from_slice(&chunk[..n]);

        if let Some(pos) = find(&buf[scanned..], b"\r\n\r\n") {
            let end = scanned + pos;
            let leftover = buf[end + 4..].to_vec();
            buf.truncate(end);
            return Ok((buf, leftover));
        }
        scanned = buf.len().saturating_sub(3);

        if buf.len() > MAX_HEADERS {
            return Err(Error::TooLarge("headers"));
        }
    }
}

pub(crate) fn parse_head(head: &[u8]) -> Result<(u16, Vec<(String, String)>), Error> {
    let text = String::from_utf8_lossy(head);
    let mut lines = text.split("\r\n");

    let status_line = lines.next().unwrap_or("");
    // "HTTP/1.1 200 OK" — the reason phrase is optional and ignored.
    let mut parts = status_line.splitn(3, ' ');
    let version = parts.next().unwrap_or("");
    if !version.starts_with("HTTP/") {
        return Err(Error::Protocol(format!("bad status line: {status_line:?}")));
    }
    let status: u16 = parts
        .next()
        .and_then(|s| s.parse().ok())
        .ok_or_else(|| Error::Protocol(format!("bad status code in {status_line:?}")))?;

    let mut headers = Vec::new();
    for line in lines {
        if line.is_empty() {
            continue;
        }
        // A header with no colon is malformed; skip it rather than failing the whole response.
        if let Some((name, value)) = line.split_once(':') {
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    Ok((status, headers))
}

fn read_body(
    transport: &mut Transport,
    headers: &[(String, String)],
    leftover: Vec<u8>,
) -> Result<Vec<u8>, Error> {
    let get = |name: &str| {
        headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case(name))
            .map(|(_, v)| v.as_str())
    };

    // Transfer-Encoding wins over Content-Length when both are present (RFC 9112 §6.3), and a
    // server sending both is a request-smuggling smell anyway.
    let chunked = get("transfer-encoding")
        .map(|v| v.to_ascii_lowercase().contains("chunked"))
        .unwrap_or(false);

    if chunked {
        return read_chunked(transport, leftover);
    }

    if let Some(len) = get("content-length").and_then(|v| v.trim().parse::<usize>().ok()) {
        if len > MAX_BODY {
            return Err(Error::TooLarge("body"));
        }
        let mut body = leftover;
        body.reserve(len.saturating_sub(body.len()));
        while body.len() < len {
            let mut chunk = [0u8; 8192];
            let want = core::cmp::min(chunk.len(), len - body.len());
            match transport.read(&mut chunk[..want]) {
                Ok(0) => break, // short body; return what arrived rather than losing it
                Ok(n) => body.extend_from_slice(&chunk[..n]),
                Err(e) if is_clean_eof(&e) => break,
                Err(e) => return Err(e.into()),
            }
        }
        return Ok(body);
    }

    // No framing at all: the body runs to EOF. Legal because we sent `Connection: close`.
    let mut body = leftover;
    let mut chunk = [0u8; 8192];
    loop {
        match transport.read(&mut chunk) {
            Ok(0) => break,
            Ok(n) => {
                body.extend_from_slice(&chunk[..n]);
                if body.len() > MAX_BODY {
                    return Err(Error::TooLarge("body"));
                }
            }
            Err(e) if is_clean_eof(&e) => break,
            Err(e) => return Err(e.into()),
        }
    }
    Ok(body)
}

/// RFC 9112 §7.1 chunked coding: `<hex-size>[;ext]CRLF <data> CRLF`, terminated by a 0-size chunk.
fn read_chunked(transport: &mut Transport, leftover: Vec<u8>) -> Result<Vec<u8>, Error> {
    let mut pending = leftover;
    let mut body = Vec::new();

    loop {
        // Chunk size line.
        let line = match take_line(transport, &mut pending)? {
            Some(l) => l,
            None => break, // truncated stream; keep what we decoded
        };
        // Strip any chunk extension after ';', then parse the hex length.
        let size_text = line.split(';').next().unwrap_or("").trim();
        let size = match usize::from_str_radix(size_text, 16) {
            Ok(s) => s,
            Err(_) => return Err(Error::Protocol(format!("bad chunk size {size_text:?}"))),
        };
        if size == 0 {
            break; // trailers may follow; we don't need them
        }
        if body.len() + size > MAX_BODY {
            return Err(Error::TooLarge("body"));
        }

        // Chunk data, plus the CRLF that follows it.
        while pending.len() < size + 2 {
            let mut chunk = [0u8; 8192];
            match transport.read(&mut chunk) {
                Ok(0) => break,
                Ok(n) => pending.extend_from_slice(&chunk[..n]),
                Err(e) if is_clean_eof(&e) => break,
                Err(e) => return Err(e.into()),
            }
        }
        let take = core::cmp::min(size, pending.len());
        body.extend_from_slice(&pending[..take]);
        // Drop the data and its trailing CRLF (saturating, in case the stream was truncated).
        let drop_to = core::cmp::min(pending.len(), size + 2);
        pending.drain(..drop_to);
        if take < size {
            break; // truncated
        }
    }
    Ok(body)
}

/// Pull one CRLF-terminated line out of `pending`, reading more if needed. `None` means EOF first.
fn take_line(transport: &mut Transport, pending: &mut Vec<u8>) -> Result<Option<String>, Error> {
    loop {
        if let Some(pos) = find(pending, b"\r\n") {
            let line = String::from_utf8_lossy(&pending[..pos]).into_owned();
            pending.drain(..pos + 2);
            return Ok(Some(line));
        }
        let mut chunk = [0u8; 4096];
        match transport.read(&mut chunk) {
            Ok(0) => return Ok(None),
            Ok(n) => pending.extend_from_slice(&chunk[..n]),
            Err(e) if is_clean_eof(&e) => return Ok(None),
            Err(e) => return Err(e.into()),
        }
        if pending.len() > MAX_HEADERS {
            return Err(Error::TooLarge("chunk header"));
        }
    }
}

/// A server that closes without a TLS `close_notify` is extremely common with `Connection: close`,
/// and rustls surfaces that as `UnexpectedEof`. Treating it as a hard error would fail most fetches.
pub(crate) fn is_clean_eof(e: &std::io::Error) -> bool {
    e.kind() == std::io::ErrorKind::UnexpectedEof
}

pub(crate) fn find(haystack: &[u8], needle: &[u8]) -> Option<usize> {
    if needle.is_empty() || haystack.len() < needle.len() {
        return None;
    }
    haystack.windows(needle.len()).position(|w| w == needle)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_status_and_headers() {
        let head = b"HTTP/1.1 301 Moved Permanently\r\nLocation: https://x/\r\nContent-Length: 0";
        let (status, headers) = parse_head(head).unwrap();
        assert_eq!(status, 301);
        assert_eq!(headers.len(), 2);
        assert_eq!(headers[0].0, "Location");
        assert_eq!(headers[0].1, "https://x/");
    }

    #[test]
    fn status_line_without_reason_phrase_is_fine() {
        assert_eq!(parse_head(b"HTTP/1.1 204").unwrap().0, 204);
    }

    #[test]
    fn rejects_non_http() {
        assert!(parse_head(b"garbage\r\n").is_err());
    }

    #[test]
    fn header_lookup_is_case_insensitive() {
        let resp = Response {
            status: 200,
            headers: vec![("Content-Type".into(), "text/html".into())],
            body: Vec::new(),
            url: Url::parse("http://x/").unwrap(),
        };
        assert_eq!(resp.header("content-type"), Some("text/html"));
        assert_eq!(resp.header("CONTENT-TYPE"), Some("text/html"));
    }

    #[test]
    fn finds_the_header_terminator() {
        assert_eq!(find(b"abc\r\n\r\ndef", b"\r\n\r\n"), Some(3));
        assert_eq!(find(b"abcdef", b"\r\n\r\n"), None);
    }
}
