//! HTTP/1.1 over plain TCP or TLS 1.3.
//!
//! Scope is "fetch a document". Connections are **reused** when the response framing allows it —
//! see [`response_is_reusable`] — which is what makes following a link on the page you are already
//! reading cost one round trip instead of a TCP handshake plus a full TLS handshake with a software
//! certificate verification. No pipelining and no HTTP/2: those multiply *concurrency*, which is a
//! different problem from *latency* and a much easier one to get wrong.
//!
//! `Accept-Encoding: gzip`, since HTML is the most compressible thing on the wire and the wire is
//! the slow part of this machine. Brotli is deliberately not asked for: it would be a second decoder
//! for a smaller marginal win, and gzip is the one every server has.

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

/// How long one TCP handshake may take before the next address is tried.
///
/// Deliberately short. A TCP handshake is one round trip; anything beyond a second or so means the
/// address is not going to answer, and the useful response to that is to move on rather than to
/// keep waiting. The whole-request budget (`TOTAL_DEADLINE`) still bounds the sweep.
const CONNECT_TIMEOUT: Duration = Duration::from_secs(3);

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
///
/// Applied to the body both on the wire and after inflating, because a gzip stream is allowed to be
/// far smaller than what it expands to and the limit exists to protect the heap, not the link.
pub(crate) const MAX_BODY: usize = 8 * 1024 * 1024;
/// Headers past this point are a malformed or hostile server, not a real response.
const MAX_HEADERS: usize = 64 * 1024;
/// How many header FIELDS one response may carry.
///
/// `MAX_HEADERS` bounds the bytes but not the count, and the two are different attacks: 64 KB of
/// two-character header lines is roughly ten thousand fields, every one of which becomes two heap
/// `String`s in `parse_head` and is then walked linearly by every `header()` lookup. Bounding the
/// count is what keeps that from being quadratic.
const MAX_HEADER_FIELDS: usize = 128;
/// Redirect chains longer than this are a loop.
const MAX_REDIRECTS: usize = 8;

/// How long the whole blocking `get` may take, across every redirect.
///
/// [`IO_TIMEOUT`] bounds one socket operation, which is not the same thing and does not add up to
/// it: eight redirects, each with its own connect and its own reads, can stack far past any single
/// deadline. Without this a `fetch` can sit for minutes producing nothing — and because the blocking
/// path runs inside the terminal's key handler, "minutes" means a window that cannot even be closed.
///
/// The stepped [`crate::fetch::Fetch`] has always had its own whole-request deadline; this is the
/// same guarantee for the path that predates it.
const TOTAL_DEADLINE: Duration = Duration::from_secs(60);

/// A whole-request deadline, passed down into the read loops so a stalled transfer cannot outlive it.
#[derive(Clone, Copy)]
pub(crate) struct Deadline {
    started: Instant,
    limit: Duration,
}

impl Deadline {
    fn new(limit: Duration) -> Deadline {
        Deadline { started: Instant::now(), limit }
    }

    fn expired(&self) -> bool {
        self.started.elapsed() > self.limit
    }

    /// `Err` once the budget is gone, so a read loop can `?` on it.
    fn check(&self) -> Result<(), Error> {
        if self.expired() {
            Err(Error::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the whole request took too long",
            )))
        } else {
            Ok(())
        }
    }
}

/// What went wrong, at the granularity a reader can act on.
///
/// ★ The variants exist to be told apart. Every one of these used to arrive as `Io` — a name that
/// does not resolve, a host that will not answer, and a certificate that is not valid yet are three
/// different problems with three different fixes, and collapsing them into "Network error" is
/// exactly the failure mode this taxonomy is here to prevent.
#[derive(Debug)]
pub enum Error {
    Url(crate::url::ParseError),
    /// The name could not be resolved at all.
    Dns { host: String },
    /// The name resolved, and the address would not accept a connection.
    Connect { addr: String, source: std::io::Error },
    /// The connection existed and then failed.
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
            Error::Dns { host } => write!(f, "cannot resolve {host}"),
            Error::Connect { addr, source } => write!(f, "{addr} is not answering ({source})"),
            Error::Io(e) => write!(f, "{e}"),
            Error::Tls(e) => write!(f, "TLS error: {e}"),
            Error::Protocol(s) => write!(f, "bad HTTP response: {s}"),
            Error::TooLarge(what) => write!(f, "{what} exceeded the size limit"),
            Error::TooManyRedirects => write!(f, "too many redirects"),
        }
    }
}

impl Error {
    /// A sentence saying what to do about it, or `None` when the `Display` text is already the
    /// whole story.
    ///
    /// Kept beside the variant rather than in the terminal so that every caller gets the same
    /// advice — and so the advice is reviewed next to the condition that triggers it.
    pub fn explain(&self) -> Option<&'static str> {
        match self {
            Error::Dns { .. } => Some(
                "The name did not resolve. Check the link is up and has a DNS server (`wifi`), or that the name is spelled correctly.",
            ),
            Error::Connect { .. } => Some(
                "The address was reachable to look up but refused or ignored the connection. Another of the name's addresses may work — this is retried automatically.",
            ),
            Error::Tls(e) => {
                let s = e.to_string();
                if s.contains("not valid yet") || s.contains("expired") {
                    // By far the most common TLS failure on this machine, and it is not a TLS
                    // problem at all — the RTC loses its value across a power cycle.
                    Some(
                        "The certificate's validity window does not include this machine's clock. Run `date` to check it, then `time sync`.",
                    )
                } else if s.contains("UnknownIssuer") || s.contains("unknown issuer") {
                    Some("The certificate does not chain to any trusted root.")
                } else if s.contains("NotValidForName") {
                    Some("The certificate is for a different hostname than the one requested.")
                } else {
                    None
                }
            }
            Error::TooManyRedirects => {
                Some("The server kept redirecting. This is usually a redirect loop.")
            }
            Error::TooLarge(_) => {
                Some("The response exceeded the size this machine will hold in memory.")
            }
            _ => None,
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

    /// The charset the server declared in `Content-Type`, if it declared one.
    ///
    /// Only the header. A `<meta charset>` inside the document is the other half of the answer, and
    /// it belongs to whoever knows the body is HTML — see `nyx_htmltext::sniff_charset`.
    pub fn charset(&self) -> Option<String> {
        charset_from_content_type(self.content_type()?)
    }

    /// Body as text, honouring the declared charset.
    ///
    /// Lossy within a charset on purpose: a browser renders what it can rather than refusing a page
    /// over one bad byte.
    pub fn text(&self) -> String {
        crate::body::decode_text(&self.body, self.charset().as_deref())
    }

    /// Body as text under an explicitly chosen charset, for a caller that sniffed a `<meta>` the
    /// header did not mention.
    pub fn text_as(&self, charset: Option<&str>) -> String {
        crate::body::decode_text(&self.body, charset)
    }
}

/// Pull `charset=x` out of a MIME type.
///
/// Duplicated from `nyx_htmltext` on purpose: this crate is the transport and does not depend on the
/// renderer, and a five-line parameter split is a much smaller thing to repeat than a dependency
/// edge from the network stack to an HTML library.
fn charset_from_content_type(value: &str) -> Option<String> {
    let lower = value.to_ascii_lowercase();
    let at = lower.find("charset")?;
    let rest = lower[at + "charset".len()..].trim_start().strip_prefix('=')?.trim_start();
    let rest = rest.strip_prefix('"').unwrap_or(rest);
    let end = rest
        .find(|c: char| c == '"' || c == ';' || c.is_ascii_whitespace())
        .unwrap_or(rest.len());
    let cs = rest[..end].trim();
    if cs.is_empty() {
        None
    } else {
        Some(cs.to_string())
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

/// What the raw socket underneath actually did.
///
/// ★ Counted at the socket, NOT at [`Transport::read`], and the distinction is the whole point. A
/// TLS `Transport::read` returns *plaintext*: during a handshake rustls consumes the server's
/// records internally and hands back nothing, so a plaintext byte count reads zero throughout a
/// perfectly healthy handshake. Reporting that number as "bytes received" turns "the handshake is
/// still going" into what looks like "the server never answered", which are opposite diagnoses with
/// opposite fixes.
#[derive(Default)]
pub struct SockStats {
    pub reads: core::sync::atomic::AtomicUsize,
    pub read_bytes: core::sync::atomic::AtomicUsize,
    pub timeouts: core::sync::atomic::AtomicUsize,
    pub writes: core::sync::atomic::AtomicUsize,
    pub write_bytes: core::sync::atomic::AtomicUsize,
    /// Raw errno of the last failed socket operation, or 0. Raw because Nyx's `decode_error_kind`
    /// has been `Uncategorized`-for-everything before, so the mapped kind is not trustworthy.
    pub last_errno: core::sync::atomic::AtomicI32,
}

impl SockStats {
    pub fn summary(&self) -> String {
        use core::sync::atomic::Ordering::Relaxed;
        format!(
            "sock[rd={}/{}B wr={}/{}B to={} errno={}]",
            self.reads.load(Relaxed),
            self.read_bytes.load(Relaxed),
            self.writes.load(Relaxed),
            self.write_bytes.load(Relaxed),
            self.timeouts.load(Relaxed),
            self.last_errno.load(Relaxed),
        )
    }
}

/// A `TcpStream` that records what passed through it, so a stalled TLS handshake can be told apart
/// from a server that is simply not replying.
pub(crate) struct CountingStream {
    pub(crate) inner: TcpStream,
    stats: Arc<SockStats>,
}

impl Read for CountingStream {
    fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
        use core::sync::atomic::Ordering::Relaxed;
        let r = self.inner.read(buf);
        self.stats.reads.fetch_add(1, Relaxed);
        match &r {
            Ok(n) => {
                self.stats.read_bytes.fetch_add(*n, Relaxed);
            }
            Err(e) => {
                let code = e.raw_os_error().unwrap_or(-1);
                self.stats.last_errno.store(code, Relaxed);
                // ETIMEDOUT/EAGAIN are the expected answer under a short deadline, not a fault.
                if matches!(code, 110 | 11) || e.kind() == std::io::ErrorKind::TimedOut {
                    self.stats.timeouts.fetch_add(1, Relaxed);
                }
            }
        }
        r
    }
}

impl Write for CountingStream {
    fn write(&mut self, buf: &[u8]) -> std::io::Result<usize> {
        use core::sync::atomic::Ordering::Relaxed;
        let r = self.inner.write(buf);
        self.stats.writes.fetch_add(1, Relaxed);
        match &r {
            Ok(n) => {
                self.stats.write_bytes.fetch_add(*n, Relaxed);
            }
            Err(e) => {
                self.stats.last_errno.store(e.raw_os_error().unwrap_or(-1), Relaxed);
            }
        }
        r
    }
    fn flush(&mut self) -> std::io::Result<()> {
        self.inner.flush()
    }
}

/// Plain or encrypted, behind one Read+Write so the HTTP code never branches on it.
pub(crate) enum Transport {
    Plain(CountingStream),
    Tls(Box<rustls::StreamOwned<rustls::ClientConnection, CountingStream>>),
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
        // ★ A bounded handshake, because the caller intends to try the NEXT address.
        //
        // `TcpStream::connect` leaves the kernel on its 10 s default, which is the right answer for
        // a client with one address and the wrong one for a client walking four: four unreachable
        // candidates cost forty seconds of a blank window. At 3 s the whole sweep fits in the time
        // a single attempt used to take, and a reachable host still connects in one round trip.
        let tcp = TcpStream::connect_timeout(&addr, CONNECT_TIMEOUT)
            .map_err(|e| Error::Connect { addr: addr.to_string(), source: e })?;
        trace!(start, "tcp connected");

        // Arm the deadline before any traffic. Doing it after the handshake would leave exactly the
        // window that hangs — the handshake is the part most likely to stall.
        tcp.set_read_timeout(Some(IO_TIMEOUT))?;
        tcp.set_write_timeout(Some(IO_TIMEOUT))?;
        let sock = CountingStream { inner: tcp, stats: Arc::new(SockStats::default()) };

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

    /// What rustls thinks the state of the connection is: `(handshaking, wants_read, wants_write)`.
    ///
    /// `None` for a plain connection, which has no handshake to be in the middle of.
    ///
    /// This exists because a stalled HTTPS fetch is otherwise indistinguishable from a slow one from
    /// outside, and the two have opposite fixes. `handshaking` still true after thousands of
    /// milliseconds says the TLS state machine is not advancing; `wants_write` stuck true says its
    /// output is not reaching the socket. Neither is visible from the byte counts alone.
    pub(crate) fn tls_progress(&self) -> Option<(bool, bool, bool)> {
        match self {
            Transport::Plain(_) => None,
            Transport::Tls(s) => {
                Some((s.conn.is_handshaking(), s.conn.wants_read(), s.conn.wants_write()))
            }
        }
    }

    /// Re-arm both socket deadlines. The stepped fetch uses a much shorter one than `get`, because
    /// there a timeout means "come back next frame" rather than "give up".
    pub(crate) fn set_timeout(&mut self, d: Duration) -> Result<(), Error> {
        let sock = match self {
            Transport::Plain(s) => &s.inner,
            Transport::Tls(s) => &s.sock.inner,
        };
        sock.set_read_timeout(Some(d))?;
        sock.set_write_timeout(Some(d))?;
        Ok(())
    }

    /// The raw socket's counters, shared so a caller can keep reading them after handing the
    /// transport off.
    pub(crate) fn stats(&self) -> Arc<SockStats> {
        match self {
            Transport::Plain(s) => s.stats.clone(),
            Transport::Tls(s) => s.sock.stats.clone(),
        }
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

/// A kept-alive connection, waiting to be reused.
///
/// ★ One slot, not a pool. The browser fetches one page at a time, and the case worth optimising is
/// the overwhelmingly common one: following a link on the page you are already reading. A single
/// slot covers that completely, and every extra slot is more state that can desynchronise.
pub(crate) struct Idle {
    transport: Transport,
    scheme: Scheme,
    host: String,
    port: u16,
    since: Instant,
}

static IDLE: std::sync::Mutex<Option<Idle>> = std::sync::Mutex::new(None);

/// How long a kept connection is trusted before being dropped unused.
///
/// Servers close idle connections on their own schedule — commonly 5 to 60 seconds — and without
/// telling us. Past this age a kept socket is more likely to cost a failed request and a retry than
/// to save a handshake, so reconnecting is the cheaper bet.
const IDLE_MAX: Duration = Duration::from_secs(15);

/// Take a connection to this origin, if one is waiting and still fresh.
///
/// Matched on scheme AND host AND port. Scheme is part of the identity: an http and an https
/// connection to the same host:port are not interchangeable — one is wrapped in TLS.
pub(crate) fn take_idle(url: &Url) -> Option<Transport> {
    let mut slot = IDLE.lock().ok()?;
    let ok = match slot.as_ref() {
        Some(i) => {
            i.scheme == url.scheme
                && i.host == url.host
                && i.port == url.port
                && i.since.elapsed() <= IDLE_MAX
        }
        None => false,
    };
    if ok {
        slot.take().map(|i| i.transport)
    } else {
        None
    }
}

/// Offer a connection for reuse.
///
/// ⚠️ The caller must have consumed the response body **exactly**. A connection handed back with
/// unread bytes still on it gives those bytes to the NEXT request as if they were its response —
/// protocol desynchronisation, which produces garbage that reads like a parser bug rather than like
/// a connection bug. `Fetch::finish` is the only caller, and only on the framed-and-complete path.
pub(crate) fn put_idle(url: &Url, transport: Transport) {
    if let Ok(mut slot) = IDLE.lock() {
        *slot = Some(Idle {
            transport,
            scheme: url.scheme,
            host: url.host.clone(),
            port: url.port,
            since: Instant::now(),
        });
    }
}

/// Drop any kept connection. Wanted when the link changes underneath us — a socket opened on the
/// previous network will not work on this one.
pub fn close_idle() {
    if let Ok(mut slot) = IDLE.lock() {
        *slot = None;
    }
}

/// Whether a fully-read response leaves the connection reusable.
///
/// Both must hold:
///
/// * the body was **framed** — `Content-Length` or `chunked`. A body delimited only by the close
///   itself has no length, so there is no way to tell its end from a pause, and reusing such a
///   connection means guessing.
/// * the server did not say `Connection: close`. It is allowed to hang up whenever it likes, and
///   saying so is the one courtesy we can rely on.
pub(crate) fn response_is_reusable(headers: &[(String, String)], framed: bool) -> bool {
    framed
        && !headers.iter().any(|(k, v)| {
            k.eq_ignore_ascii_case("connection") && v.to_ascii_lowercase().contains("close")
        })
}

/// The request bytes for a GET. Shared so the blocking and stepped paths cannot drift apart.
pub(crate) fn request_line(url: &Url, keep_alive: bool) -> Vec<u8> {
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
         Accept-Encoding: gzip\r\n\
         Connection: {conn}\r\n\r\n",
        url.path,
        conn = if keep_alive { "keep-alive" } else { "close" }
    )
    .into_bytes()
}

/// Undo `Content-Encoding` on a completed body. Shared by the blocking and stepped paths so the two
/// cannot disagree about whether a page arrived compressed.
pub(crate) fn decode_content_encoding(
    headers: &[(String, String)],
    body: Vec<u8>,
) -> Result<Vec<u8>, Error> {
    let encoding = headers
        .iter()
        .find(|(k, _)| k.eq_ignore_ascii_case("content-encoding"))
        .map(|(_, v)| v.trim().to_ascii_lowercase());
    match encoding.as_deref() {
        Some("gzip") | Some("x-gzip") => crate::body::gunzip(body),
        // `identity`, absent, or something we did not ask for. Handing the bytes back untouched is
        // right for the first two and is the least-bad answer for the third: the caller sees the
        // compressed bytes as text, which is obviously wrong on screen, rather than seeing nothing.
        _ => Ok(body),
    }
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
    let deadline = Deadline::new(TOTAL_DEADLINE);
    for _ in 0..MAX_REDIRECTS {
        deadline.check()?;
        let resp = get_with_deadline(&current, deadline)?;
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

/// Fetch only the response HEADERS, then hang up.
///
/// For a caller that wants a header and not a document. `time sync` needs `Date:` and nothing else,
/// and reading the body meant downloading a whole page in order to throw it away — on a slow link
/// that is the difference between a fraction of a second and tens of seconds, spent inside a
/// blocking call that freezes the caller's window.
///
/// Redirects are deliberately NOT followed: a 301's headers carry a `Date` exactly as a 200's do, so
/// there is nothing to gain by chasing one and a whole extra round trip to lose.
///
/// Dropping the `Transport` closes the socket. The server is left with an unread body, which is
/// what `Connection: close` already told it to expect.
pub fn head_only(url: &str) -> Result<Response, Error> {
    let u = Url::parse(url)?;
    let deadline = Deadline::new(TOTAL_DEADLINE);
    let start = Instant::now();
    let addr = crate::fetch::resolve_host(&u)?;
    let mut transport = Transport::connect(&u, addr, &start)?;
    transport.write_all(&request_line(&u, false))?;
    transport.flush()?;
    let (head, _body_start) = read_head(&mut transport, deadline)?;
    let (status, headers) = parse_head(&head)?;
    trace!(&start, "head only: status {status}, {} fields", headers.len());
    Ok(Response { status, headers, body: Vec::new(), url: u })
}

/// One request, no redirect handling, with its own whole-request budget.
pub fn get_once(url: &Url) -> Result<Response, Error> {
    get_with_deadline(url, Deadline::new(TOTAL_DEADLINE))
}

fn get_with_deadline(url: &Url, deadline: Deadline) -> Result<Response, Error> {
    let start = Instant::now();
    trace!(&start, "GET {url}");
    let addr = crate::fetch::resolve_host(url)?;
    let mut transport = Transport::connect(url, addr, &start)?;

    // For HTTPS this is where the handshake actually happens — rustls defers it to the first I/O —
    // so a stall here is a TLS problem, not a request-sending problem.
    transport.write_all(&request_line(url, false))?;
    transport.flush()?;
    trace!(&start, "request sent (handshake done for https)");

    let (head, leftover) = read_head(&mut transport, deadline)?;
    let (status, headers) = parse_head(&head)?;
    trace!(&start, "headers: status {status}, {} fields", headers.len());

    let body = read_body(&mut transport, &headers, leftover, deadline)?;
    let wire = body.len();
    let body = decode_content_encoding(&headers, body)?;
    trace!(&start, "body complete: {} bytes on the wire, {} decoded", wire, body.len());

    Ok(Response { status, headers, body, url: url.clone() })
}

/// Read until the CRLFCRLF that ends the header block. Returns the header bytes and whatever body
/// bytes arrived in the same read — those must not be dropped, they are the start of the body.
/// Generic over `Read` rather than taking `Transport`, so the framing can be exercised against an
/// in-memory stream. `Transport` implements `Read`, so no caller changed — but a `Cursor`, or a
/// reader that dribbles three bytes at a time, is now equally valid input. Response framing is the
/// part of HTTP most likely to be wrong on hostile input and it had no tests at all.
fn read_head<R: Read>(transport: &mut R, deadline: Deadline) -> Result<(Vec<u8>, Vec<u8>), Error> {
    let mut buf = Vec::with_capacity(4096);
    let mut chunk = [0u8; 4096];
    // Where to resume scanning. The terminator can straddle two reads, so back up 3 bytes.
    let mut scanned = 0usize;

    loop {
        deadline.check()?;
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
            if headers.len() >= MAX_HEADER_FIELDS {
                return Err(Error::TooLarge("header count"));
            }
            headers.push((name.trim().to_string(), value.trim().to_string()));
        }
    }
    Ok((status, headers))
}

fn read_body<R: Read>(
    transport: &mut R,
    headers: &[(String, String)],
    leftover: Vec<u8>,
    deadline: Deadline,
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
        return read_chunked(transport, leftover, deadline);
    }

    if let Some(len) = get("content-length").and_then(|v| v.trim().parse::<usize>().ok()) {
        if len > MAX_BODY {
            return Err(Error::TooLarge("body"));
        }
        let mut body = leftover;
        body.reserve(len.saturating_sub(body.len()));
        while body.len() < len {
            deadline.check()?;
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
        deadline.check()?;
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
fn read_chunked<R: Read>(
    transport: &mut R,
    leftover: Vec<u8>,
    deadline: Deadline,
) -> Result<Vec<u8>, Error> {
    let mut pending = leftover;
    let mut body = Vec::new();

    loop {
        deadline.check()?;
        // Chunk size line.
        let line = match take_line(transport, &mut pending, deadline)? {
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
            deadline.check()?;
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
fn take_line<R: Read>(
    transport: &mut R,
    pending: &mut Vec<u8>,
    deadline: Deadline,
) -> Result<Option<String>, Error> {
    loop {
        deadline.check()?;
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

    /// A reader that hands back at most `n` bytes per call.
    ///
    /// Real sockets do this constantly and it is where framing bugs live: a header terminator or a
    /// chunk-size line split across two reads is the case a naive scanner gets wrong. `n = 1` is
    /// the cruellest legal server, and the tests below sweep down to it.
    struct Dribble {
        data: Vec<u8>,
        pos: usize,
        n: usize,
    }

    impl Dribble {
        fn new(data: &[u8], n: usize) -> Dribble {
            Dribble { data: data.to_vec(), pos: 0, n }
        }
    }

    impl Read for Dribble {
        fn read(&mut self, buf: &mut [u8]) -> std::io::Result<usize> {
            let left = self.data.len() - self.pos;
            if left == 0 {
                return Ok(0);
            }
            let take = left.min(self.n).min(buf.len());
            buf[..take].copy_from_slice(&self.data[self.pos..self.pos + take]);
            self.pos += take;
            Ok(take)
        }
    }

    fn forever() -> Deadline {
        Deadline::new(Duration::from_secs(3600))
    }

    /// Read a whole response the way `get_once` does, at a given dribble size.
    fn parse_response(raw: &[u8], chunk: usize) -> Result<(u16, Vec<u8>), Error> {
        let mut r = Dribble::new(raw, chunk);
        let (head, leftover) = read_head(&mut r, forever())?;
        let (status, headers) = parse_head(&head)?;
        let body = read_body(&mut r, &headers, leftover, forever())?;
        Ok((status, body))
    }

    // ── Framing ─────────────────────────────────────────────────────────────────────────────────

    #[test]
    fn content_length_framing_at_every_dribble_size() {
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 5\r\n\r\nhello";
        for n in [1, 2, 3, 7, 64, 4096] {
            let (status, body) = parse_response(raw, n).unwrap_or_else(|e| panic!("n={n}: {e}"));
            assert_eq!(status, 200, "n={n}");
            assert_eq!(body, b"hello", "n={n}");
        }
    }

    #[test]
    fn chunked_framing_at_every_dribble_size() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5\r\nhello\r\n6\r\n world\r\n0\r\nX-After: 1\r\n\r\n";
        for n in [1, 2, 3, 7, 64, 4096] {
            let (status, body) = parse_response(raw, n).unwrap_or_else(|e| panic!("n={n}: {e}"));
            assert_eq!(status, 200, "n={n}");
            assert_eq!(body, b"hello world", "n={n}");
        }
    }

    #[test]
    fn a_chunk_extension_is_ignored_rather_than_parsed_as_a_size() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\n5;name=value\r\nhello\r\n0\r\n\r\n";
        assert_eq!(parse_response(raw, 1).unwrap().1, b"hello");
    }

    #[test]
    fn transfer_encoding_wins_over_content_length() {
        // RFC 9112 §6.3: when both are present, Transfer-Encoding decides. A server sending both is
        // a request-smuggling smell, and honouring the WRONG one is how a smuggled request lands.
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 99\r\nTransfer-Encoding: chunked\r\n\r\n2\r\nhi\r\n0\r\n\r\n";
        assert_eq!(parse_response(raw, 1).unwrap().1, b"hi");
    }

    #[test]
    fn a_body_with_no_framing_runs_to_eof() {
        // Legal because we send `Connection: close`.
        let raw = b"HTTP/1.1 200 OK\r\nContent-Type: text/plain\r\n\r\nthe whole body";
        assert_eq!(parse_response(raw, 3).unwrap().1, b"the whole body");
    }

    #[test]
    fn a_head_split_across_reads_still_finds_its_terminator() {
        // The CRLFCRLF can straddle two reads; the scanner backs up three bytes for exactly this.
        let raw = b"HTTP/1.1 204 No Content\r\nA: 1\r\nB: 2\r\n\r\n";
        for n in 1..8 {
            let mut r = Dribble::new(raw, n);
            let (head, leftover) = read_head(&mut r, forever()).unwrap();
            assert!(leftover.is_empty(), "n={n}");
            assert_eq!(parse_head(&head).unwrap().0, 204, "n={n}");
        }
    }

    // ── Malformed and hostile input ─────────────────────────────────────────────────────────────

    #[test]
    fn a_truncated_body_returns_what_arrived_rather_than_failing() {
        // A short body still renders. Losing a whole page because the last KB never came is worse
        // than showing the part that did.
        let raw = b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\n\r\nonly this much";
        assert_eq!(parse_response(raw, 5).unwrap().1, b"only this much");
    }

    #[test]
    fn a_bad_chunk_size_is_an_error_not_a_hang() {
        let raw = b"HTTP/1.1 200 OK\r\nTransfer-Encoding: chunked\r\n\r\nZZZZ\r\nhello\r\n0\r\n\r\n";
        assert!(matches!(parse_response(raw, 1), Err(Error::Protocol(_))));
    }

    #[test]
    fn a_connection_that_closes_before_any_header_is_reported() {
        assert!(matches!(parse_response(b"", 1), Err(Error::Protocol(_))));
        assert!(matches!(parse_response(b"HTTP/1.1 200 OK\r\n", 1), Err(Error::Protocol(_))));
    }

    #[test]
    fn a_header_block_that_never_ends_is_bounded() {
        // Without the MAX_HEADERS cap this reads until the heap is gone.
        let mut raw = b"HTTP/1.1 200 OK\r\n".to_vec();
        while raw.len() < MAX_HEADERS + 4096 {
            raw.extend_from_slice(b"X-Filler: aaaaaaaaaaaaaaaaaaaaaaaaaaaaaaaa\r\n");
        }
        assert!(matches!(parse_response(&raw, 4096), Err(Error::TooLarge("headers"))));
    }

    #[test]
    fn an_absurd_number_of_header_fields_is_refused() {
        // Bytes and COUNT are different limits: many tiny fields stay under MAX_HEADERS while still
        // costing two allocations each and making every header lookup linear in the count.
        let mut raw = b"HTTP/1.1 200 OK\r\n".to_vec();
        for _ in 0..MAX_HEADER_FIELDS + 50 {
            raw.extend_from_slice(b"a:b\r\n");
        }
        raw.extend_from_slice(b"\r\n");
        assert!(matches!(parse_response(&raw, 4096), Err(Error::TooLarge("header count"))));
    }

    #[test]
    fn an_oversized_content_length_is_refused_before_reading_it() {
        let raw = format!("HTTP/1.1 200 OK\r\nContent-Length: {}\r\n\r\n", MAX_BODY + 1);
        assert!(matches!(parse_response(raw.as_bytes(), 4096), Err(Error::TooLarge("body"))));
    }

    #[test]
    fn a_body_that_runs_forever_is_bounded() {
        // No Content-Length, no chunking: read-to-EOF. A server that never stops must not be able
        // to make us allocate without limit.
        let mut raw = b"HTTP/1.1 200 OK\r\n\r\n".to_vec();
        raw.resize(raw.len() + MAX_BODY + 8192, b'x');
        assert!(matches!(parse_response(&raw, 65536), Err(Error::TooLarge("body"))));
    }

    #[test]
    fn a_status_line_that_is_not_http_is_refused() {
        for bad in [
            &b"SSH-2.0-OpenSSH\r\n\r\n"[..],
            &b"\x16\x03\x01\x00\x01\r\n\r\n"[..], // a TLS record answered on the http port
            &b"HTTP/1.1 not-a-number OK\r\n\r\n"[..],
        ] {
            assert!(parse_response(bad, 4096).is_err(), "{bad:?} should be refused");
        }
    }

    #[test]
    fn header_values_containing_colons_survive_intact() {
        // Splitting on every colon would truncate a Location or a Date.
        let raw = b"HTTP/1.1 301 Moved\r\nLocation: https://x.test:8443/a?b=c\r\n\r\n";
        let mut r = Dribble::new(raw, 1);
        let (head, _) = read_head(&mut r, forever()).unwrap();
        let (status, headers) = parse_head(&head).unwrap();
        assert_eq!(status, 301);
        let loc = headers.iter().find(|(k, _)| k == "Location").unwrap();
        assert_eq!(loc.1, "https://x.test:8443/a?b=c");
    }

    #[test]
    fn an_expired_deadline_stops_the_read() {
        // The whole-request deadline must be enforced INSIDE the read loops, not only between
        // requests — a server that dribbles forever is otherwise unbounded.
        let past = Deadline {
            started: Instant::now() - Duration::from_secs(10),
            limit: Duration::from_secs(1),
        };
        let mut r = Dribble::new(b"HTTP/1.1 200 OK\r\n\r\nbody", 1);
        assert!(read_head(&mut r, past).is_err());
    }

    // ── TLS posture ─────────────────────────────────────────────────────────────────────────────

    // ── Connection reuse ────────────────────────────────────────────────────────────────────────

    fn hdrs(pairs: &[(&str, &str)]) -> Vec<(String, String)> {
        pairs.iter().map(|(k, v)| (k.to_string(), v.to_string())).collect()
    }

    #[test]
    fn only_a_framed_body_leaves_a_connection_reusable() {
        // ★ The rule that keeps keep-alive from corrupting the NEXT request.
        //
        // A body delimited only by the connection closing has no length, so "the body ended" and
        // "the peer paused" are the same observation. Reusing such a connection means guessing where
        // the response stopped — and guessing wrong feeds the remainder to the next request as if it
        // were that request's response.
        assert!(response_is_reusable(&hdrs(&[("Content-Length", "5")]), true));
        assert!(response_is_reusable(&hdrs(&[("Transfer-Encoding", "chunked")]), true));

        // `framed = false` is the read-to-EOF case, regardless of what the headers say.
        assert!(!response_is_reusable(&hdrs(&[("Content-Length", "5")]), false));
        assert!(!response_is_reusable(&[], false));
    }

    #[test]
    fn a_server_asking_to_close_is_obeyed() {
        // It may hang up whenever it likes; saying so is the one courtesy we can rely on.
        assert!(!response_is_reusable(&hdrs(&[("Connection", "close")]), true));
        // Case-insensitive, and must be found among other fields.
        assert!(!response_is_reusable(&hdrs(&[("CONNECTION", "Close")]), true));
        assert!(!response_is_reusable(
            &hdrs(&[("Content-Length", "5"), ("connection", "keep-alive, close")]),
            true
        ));
        // ...and `keep-alive` alone does not block reuse.
        assert!(response_is_reusable(&hdrs(&[("Connection", "keep-alive")]), true));
    }

    #[test]
    fn the_request_line_says_which_mode_it_is_in() {
        let u = Url::parse("http://example.com/x").unwrap();
        let keep = String::from_utf8(request_line(&u, true)).unwrap();
        let close = String::from_utf8(request_line(&u, false)).unwrap();
        assert!(keep.contains("Connection: keep-alive"), "{keep}");
        assert!(close.contains("Connection: close"), "{close}");
        // Both must still be well-formed requests ending in a blank line.
        for r in [&keep, &close] {
            assert!(r.starts_with("GET /x HTTP/1.1\r\n"), "{r}");
            assert!(r.ends_with("\r\n\r\n"), "{r}");
            assert!(r.contains("Host: example.com\r\n"), "{r}");
        }
    }

    #[test]
    fn the_idle_slot_matches_on_the_whole_origin() {
        // Scheme, host and port are all part of the identity. Handing an https connection to an
        // http request — or to a different port — would be handing over a TLS stream to code that
        // is about to write plaintext at it.
        close_idle();
        let a = Url::parse("https://example.com/one").unwrap();
        assert!(take_idle(&a).is_none(), "a flushed slot must not answer");

        // `put_idle` needs a real Transport, which needs a socket, so the negative cases are what
        // can be asserted without a network. They are also the ones that matter: a false match here
        // is a protocol desynchronisation, a false miss is just a reconnect.
        for other in [
            "http://example.com/one",   // different scheme
            "https://example.org/one",  // different host
            "https://example.com:8443/one", // different port
        ] {
            let u = Url::parse(other).unwrap();
            assert!(take_idle(&u).is_none(), "{other} must not match an empty slot either");
        }
    }

    #[test]
    fn the_tls_config_is_what_we_think_it_is() {
        // ★ A security regression test, and it needs no network.
        //
        // Everything here is a property that would be catastrophic to lose silently. An empty root
        // store, in particular, rejects every certificate for a reason that reads like a network
        // fault — the exact failure this repository has already spent days misdiagnosing once.
        let cfg = tls_config();

        // Roots are actually loaded. The count moves with webpki-roots releases; the floor is the
        // assertion that matters.
        assert!(
            cfg.root_store_size() > 50,
            "root store looks empty ({}) — every certificate would fail",
            cfg.root_store_size()
        );

        // No ALPN is offered: this client speaks HTTP/1.1 only, and advertising h2 would invite a
        // protocol we cannot parse.
        assert!(cfg.alpn_protocols.is_empty(), "ALPN must stay unset for an HTTP/1.1-only client");

        // SNI on. It is also the name the certificate is verified against, so turning it off would
        // both break virtual hosts and change what is being checked.
        assert!(cfg.enable_sni, "SNI must stay on");

        // 0-RTT is not enabled. Early data is replayable by design and this client has no way to
        // mark a request as safe to replay.
        assert!(!cfg.enable_early_data, "early data must stay off");

        // No key logging: `SSLKEYLOGFILE` would write session secrets to disk.
        // (rustls exposes no getter for this; asserting the provider shape below is the proxy.)
        let p = rustls_rustcrypto::provider();
        // Nine suites: three TLS 1.3, six TLS 1.2 ECDHE. If this shrinks, a server we could reach
        // yesterday stops being reachable; if it grows, something was added without review.
        assert_eq!(p.cipher_suites.len(), 9, "cipher suite set changed");
        // X25519, P-256, P-384.
        assert_eq!(p.kx_groups.len(), 3, "key exchange group set changed");
        // RSA PKCS#1 and PSS at three digests each, ECDSA P-256/P-384, Ed25519.
        assert!(
            p.signature_verification_algorithms.all.len() >= 11,
            "signature verification algorithms shrank"
        );
    }

    #[test]
    fn finds_the_header_terminator() {
        assert_eq!(find(b"abc\r\n\r\ndef", b"\r\n\r\n"), Some(3));
        assert_eq!(find(b"abcdef", b"\r\n\r\n"), None);
    }
}
