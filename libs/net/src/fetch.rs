//! A stepped HTTP GET: the same request as [`crate::http::get`], but driven a slice at a time.
//!
//! Nyx has no threads, so a browser that calls a blocking `get()` is a browser whose window is dead
//! for the whole transfer — and on a 500 KB page over TLS that reads as a freeze, with no way to
//! tell a slow server from a hung one. [`Fetch`] exists so the caller can pump it once per frame,
//! draw a progress bar, and stay responsive.
//!
//! The trick is a **short socket timeout** rather than non-blocking sockets: each poll blocks for at
//! most [`SOCKET_TIMEOUT`], and a timeout is read as "nothing yet, come back next frame" instead of
//! as a failure. rustls is sans-io and keeps its buffers across a failed read, so a TLS stream
//! survives being interrupted this way — that is what makes it safe to walk away mid-record.
//!
//! The real deadline is [`TOTAL_DEADLINE`], measured across the whole request. A server that dribbles
//! one byte per second cannot hold the browser forever.

use std::io::Read;
use std::time::{Duration, Instant};

use crate::http::{Error, Response};
use crate::url::Url;

/// How long a single socket operation may block. This is the worst case for one frame, so it sets
/// the floor on how responsive the window stays while loading: 150 ms means at least ~7 repaints a
/// second even when the server has gone quiet.
const SOCKET_TIMEOUT: Duration = Duration::from_millis(150);
/// How much wall time one `poll` may spend before handing the frame back. Larger means fewer
/// repaints but better throughput.
const POLL_BUDGET: Duration = Duration::from_millis(120);
/// Whole-request deadline, across every redirect.
const TOTAL_DEADLINE: Duration = Duration::from_secs(45);

/// What a fetch is doing before any payload arrives.
///
/// Two of these — resolving and connecting — are genuinely blocking calls that cannot be sliced:
/// the kernel does DNS and the TCP handshake inside one syscall each. They are named separately so
/// that when one of them stalls, the window says *which*, instead of a single "Connecting…" that
/// covers four very different failures.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Phase {
    Resolving,
    Connecting,
    Handshake,
    Headers,
}

impl Phase {
    pub fn label(self) -> &'static str {
        match self {
            Phase::Resolving => "Looking up",
            Phase::Connecting => "Connecting to",
            Phase::Handshake => "Securing connection to",
            Phase::Headers => "Waiting for",
        }
    }
}

/// Where a fetch has got to. Everything here is meant to be renderable.
pub enum Progress {
    /// No payload yet. Carries the phase so a stall is legible from the UI alone.
    Connecting(Phase),
    /// `total` is only known when the server sent a `Content-Length`; chunked responses have none,
    /// which is why the UI must cope with an indeterminate bar.
    Receiving { got: usize, total: Option<usize> },
    Done(Box<Response>),
    Failed(Error),
}

enum Stage {
    /// DNS. Blocking, bounded by the kernel's own 5 s resolver deadline.
    Resolve,
    /// TCP. Blocking, bounded by the kernel's 10 s connect deadline.
    Connect(std::net::SocketAddr),
    Head,
    Body,
    Finished,
}

/// How the body is framed. Decided once, from the headers.
enum Framing {
    Length(usize),
    Chunked(Chunk),
    /// No framing: the body runs to EOF, which is legal because we send `Connection: close`.
    ToEof,
}

enum Chunk {
    Size,
    Data(usize),
    /// The CRLF that follows a chunk's data.
    Crlf,
    Done,
}

pub struct Fetch {
    url: Url,
    stage: Stage,
    /// Whether the caller has been told about the current stage yet. A stage that blocks must be
    /// named on one poll and executed on the next, or the label describing it never reaches the
    /// screen before the freeze it is meant to explain.
    announced: bool,
    transport: Option<crate::http::Transport>,
    redirects: usize,
    started: Instant,

    /// Bytes read from the socket but not yet interpreted. Consumed through `cursor` rather than
    /// drained, so a 64 KB read carrying many small chunks does not become quadratic.
    pending: Vec<u8>,
    cursor: usize,
    /// How far the header terminator scan has already looked.
    scanned: usize,

    status: u16,
    headers: Vec<(String, String)>,
    framing: Framing,
    body: Vec<u8>,
    total: Option<usize>,
}

impl Fetch {
    pub fn new(url: &str) -> Result<Fetch, Error> {
        Ok(Fetch::at(Url::parse(url)?))
    }

    fn at(url: Url) -> Fetch {
        Fetch {
            url,
            stage: Stage::Resolve,
            announced: false,
            transport: None,
            redirects: 0,
            started: Instant::now(),
            pending: Vec::new(),
            cursor: 0,
            scanned: 0,
            status: 0,
            headers: Vec::new(),
            framing: Framing::ToEof,
            body: Vec::new(),
            total: None,
        }
    }

    /// The URL currently being fetched, which is not the one asked for once a redirect is taken.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Do one frame's worth of work.
    pub fn poll(&mut self) -> Progress {
        if self.started.elapsed() > TOTAL_DEADLINE {
            self.stage = Stage::Finished;
            return Progress::Failed(Error::Io(std::io::Error::new(
                std::io::ErrorKind::TimedOut,
                "the server took too long",
            )));
        }
        match self.step() {
            Ok(p) => p,
            Err(e) => {
                self.stage = Stage::Finished;
                Progress::Failed(e)
            }
        }
    }

    /// Name a stage on one poll and run it on the next. Returns true if this poll is the naming one.
    fn announcing(&mut self) -> bool {
        if self.announced {
            false
        } else {
            self.announced = true;
            true
        }
    }

    fn enter(&mut self, stage: Stage) {
        self.stage = stage;
        self.announced = false;
    }

    fn step(&mut self) -> Result<Progress, Error> {
        match self.stage {
            Stage::Resolve => {
                if self.announcing() {
                    return Ok(Progress::Connecting(Phase::Resolving));
                }
                // DNS is a blocking syscall with its own 5 s deadline in the kernel; splitting it
                // out from the TCP connect is what makes "stuck looking up" distinguishable from
                // "stuck connecting", which are different faults with different fixes.
                let addr = resolve(&self.url)?;
                self.enter(Stage::Connect(addr));
                Ok(Progress::Connecting(Phase::Connecting))
            }
            Stage::Connect(addr) => {
                if self.announcing() {
                    return Ok(Progress::Connecting(Phase::Connecting));
                }
                let mut transport = crate::http::Transport::connect(&self.url, addr, &self.started)?;
                // Long enough to survive a slow first response, short enough that a dead peer costs
                // one frame. The whole-request deadline is what actually bounds a stall.
                transport.set_timeout(SOCKET_TIMEOUT)?;
                transport.queue_request(&crate::http::request_line(&self.url))?;
                self.transport = Some(transport);
                self.enter(Stage::Head);
                Ok(Progress::Connecting(self.head_phase()))
            }
            Stage::Head => {
                // EOF here is fatal and must be reported. Ignoring it left the fetch announcing
                // "Connecting" every frame against a closed socket until the 45 s whole-request
                // deadline — which reads as a hang, and hides the real fault.
                if !self.fill()? {
                    return Err(Error::Protocol("connection closed before any reply".into()));
                }
                let Some(end) = self.find_head_end() else {
                    return Ok(Progress::Connecting(self.head_phase()));
                };
                let head = self.avail()[..end].to_vec();
                self.consume(end + 4);

                let (status, headers) = crate::http::parse_head(&head)?;
                self.status = status;
                self.headers = headers;
                self.framing = self.pick_framing()?;
                self.total = match self.framing {
                    Framing::Length(n) => Some(n),
                    _ => None,
                };
                self.enter(Stage::Body);
                Ok(Progress::Receiving { got: 0, total: self.total })
            }
            Stage::Body => {
                let deadline = Instant::now();
                loop {
                    if self.decode_body()? {
                        return self.finish();
                    }
                    if deadline.elapsed() > POLL_BUDGET {
                        return Ok(Progress::Receiving {
                            got: self.body.len(),
                            total: self.total,
                        });
                    }
                    // EOF with an unfinished body is not an error: a truncated page still renders,
                    // and `Connection: close` responses end exactly this way by design.
                    if !self.fill()? {
                        return self.finish();
                    }
                }
            }
            Stage::Finished => Ok(Progress::Connecting(Phase::Headers)),
        }
    }

    /// Before the first byte of an HTTPS reply arrives we are still shaking hands — rustls defers
    /// the handshake into the first read, so "no bytes yet" and "handshaking" are the same state.
    fn head_phase(&self) -> Phase {
        if self.url.scheme == crate::url::Scheme::Https && self.pending.is_empty() {
            Phase::Handshake
        } else {
            Phase::Headers
        }
    }

    /// Hand back the response, or follow a redirect by restarting against the new URL.
    fn finish(&mut self) -> Result<Progress, Error> {
        let redirecting = matches!(self.status, 301 | 302 | 303 | 307 | 308);
        let location = self
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("location"))
            .map(|(_, v)| v.clone());

        if let (true, Some(location)) = (redirecting, location) {
            if self.redirects >= 8 {
                return Err(Error::TooManyRedirects);
            }
            let next = self.url.join(&location)?;
            let started = self.started;
            let redirects = self.redirects + 1;
            *self = Fetch::at(next);
            // Carry the clock and the count across, or a redirect loop would reset its own deadline.
            self.started = started;
            self.redirects = redirects;
            return Ok(Progress::Connecting(Phase::Resolving));
        }

        self.stage = Stage::Finished;
        Ok(Progress::Done(Box::new(Response {
            status: self.status,
            headers: core::mem::take(&mut self.headers),
            body: core::mem::take(&mut self.body),
            url: self.url.clone(),
        })))
    }

    // --- buffer plumbing ---

    fn avail(&self) -> &[u8] {
        &self.pending[self.cursor..]
    }

    fn consume(&mut self, n: usize) {
        self.cursor += n;
        // Compact only occasionally: draining on every chunk would make a large read quadratic.
        if self.cursor > 32 * 1024 {
            self.pending.drain(..self.cursor);
            self.cursor = 0;
        }
    }

    /// Read once from the socket. `false` means the peer closed; a timeout is *not* a close, it is
    /// simply this frame's answer.
    fn fill(&mut self) -> Result<bool, Error> {
        let Some(transport) = self.transport.as_mut() else { return Ok(false) };
        let mut chunk = [0u8; 16 * 1024];
        match transport.read(&mut chunk) {
            Ok(0) => Ok(false),
            Ok(n) => {
                self.pending.extend_from_slice(&chunk[..n]);
                Ok(true)
            }
            Err(e) if would_block(&e) => Ok(true),
            Err(e) if crate::http::is_clean_eof(&e) => Ok(false),
            Err(e) => Err(e.into()),
        }
    }

    fn find_head_end(&mut self) -> Option<usize> {
        let hay = self.avail();
        // The terminator can straddle two reads, so resume three bytes back.
        let from = self.scanned;
        let found = crate::http::find(&hay[from.min(hay.len())..], b"\r\n\r\n").map(|p| from + p);
        if found.is_none() {
            self.scanned = hay.len().saturating_sub(3);
        }
        found
    }

    fn pick_framing(&self) -> Result<Framing, Error> {
        let get = |name: &str| {
            self.headers
                .iter()
                .find(|(k, _)| k.eq_ignore_ascii_case(name))
                .map(|(_, v)| v.as_str())
        };
        // Transfer-Encoding wins over Content-Length when both are present (RFC 9112 §6.3).
        if get("transfer-encoding").map(|v| v.to_ascii_lowercase().contains("chunked")).unwrap_or(false)
        {
            return Ok(Framing::Chunked(Chunk::Size));
        }
        if let Some(len) = get("content-length").and_then(|v| v.trim().parse::<usize>().ok()) {
            if len > crate::http::MAX_BODY {
                return Err(Error::TooLarge("body"));
            }
            return Ok(Framing::Length(len));
        }
        Ok(Framing::ToEof)
    }

    /// Move whatever is decodable out of `pending` and into `body`. Returns true when the body is
    /// complete.
    fn decode_body(&mut self) -> Result<bool, Error> {
        match self.framing {
            Framing::Length(remaining) => {
                let take = remaining.min(self.avail().len());
                if take > 0 {
                    let bytes = self.avail()[..take].to_vec();
                    self.body.extend_from_slice(&bytes);
                    self.consume(take);
                    self.framing = Framing::Length(remaining - take);
                }
                Ok(matches!(self.framing, Framing::Length(0)))
            }
            Framing::ToEof => {
                let take = self.avail().len();
                if take > 0 {
                    let bytes = self.avail().to_vec();
                    self.body.extend_from_slice(&bytes);
                    self.consume(take);
                    if self.body.len() > crate::http::MAX_BODY {
                        return Err(Error::TooLarge("body"));
                    }
                }
                Ok(false) // only EOF ends this one
            }
            Framing::Chunked(_) => self.decode_chunked(),
        }
    }

    /// RFC 9112 §7.1 chunked coding, as a resumable state machine: `<hex-size>[;ext]CRLF <data> CRLF`,
    /// terminated by a zero-size chunk. Returning `false` means "need more bytes", not "failed".
    fn decode_chunked(&mut self) -> Result<bool, Error> {
        loop {
            let state = match &self.framing {
                Framing::Chunked(c) => c,
                _ => return Ok(true),
            };
            match *state {
                Chunk::Size => {
                    let Some(pos) = crate::http::find(self.avail(), b"\r\n") else {
                        // A size line this long is a malformed or hostile server, not a slow one.
                        if self.avail().len() > 64 * 1024 {
                            return Err(Error::TooLarge("chunk header"));
                        }
                        return Ok(false);
                    };
                    let line = String::from_utf8_lossy(&self.avail()[..pos]).into_owned();
                    self.consume(pos + 2);
                    let size_text = line.split(';').next().unwrap_or("").trim();
                    let size = usize::from_str_radix(size_text, 16)
                        .map_err(|_| Error::Protocol(format!("bad chunk size {size_text:?}")))?;
                    self.framing = if size == 0 {
                        Framing::Chunked(Chunk::Done)
                    } else {
                        Framing::Chunked(Chunk::Data(size))
                    };
                }
                Chunk::Data(n) => {
                    let take = n.min(self.avail().len());
                    if take == 0 {
                        return Ok(false);
                    }
                    let bytes = self.avail()[..take].to_vec();
                    self.body.extend_from_slice(&bytes);
                    self.consume(take);
                    if self.body.len() > crate::http::MAX_BODY {
                        return Err(Error::TooLarge("body"));
                    }
                    self.framing = if take == n {
                        Framing::Chunked(Chunk::Crlf)
                    } else {
                        Framing::Chunked(Chunk::Data(n - take))
                    };
                }
                Chunk::Crlf => {
                    if self.avail().len() < 2 {
                        return Ok(false);
                    }
                    self.consume(2);
                    self.framing = Framing::Chunked(Chunk::Size);
                }
                Chunk::Done => return Ok(true),
            }
        }
    }
}

/// A short socket timeout is how this module asks "is there anything yet?", so the timeout errors it
/// produces mean *not yet* — never *failed*. `WouldBlock` is included because a non-blocking socket
/// would say the same thing a different way.
///
/// The raw codes are checked as well as the kind, and that is not belt-and-braces: Nyx originally
/// took std's `generic` error module, whose `decode_error_kind` returns `Uncategorized` for
/// everything. That made the `kind()` test silently false, so every routine short read was treated
/// as a hard failure and no page over a slow link could ever load. The PAL has a real errno table
/// now, but this check no longer depends on it being right.
/// Resolve a URL's host to one address.
///
/// Deliberately separate from `TcpStream::connect((host, port))`, which does both in one opaque
/// blocking call. Keeping them apart costs nothing and buys the ability to say which one hung.
pub(crate) fn resolve_host(url: &Url) -> Result<std::net::SocketAddr, Error> {
    resolve(url)
}

fn resolve(url: &Url) -> Result<std::net::SocketAddr, Error> {
    use std::net::ToSocketAddrs;
    (url.host.as_str(), url.port)
        .to_socket_addrs()?
        .next()
        .ok_or_else(|| Error::Protocol(format!("{} has no address", url.host)))
}

fn would_block(e: &std::io::Error) -> bool {
    const ETIMEDOUT: i32 = 110;
    const EAGAIN: i32 = 11;
    matches!(e.kind(), std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock)
        || matches!(e.raw_os_error(), Some(ETIMEDOUT) | Some(EAGAIN))
}
