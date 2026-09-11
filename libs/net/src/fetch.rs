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

/// How long a single socket operation may block once the body is streaming.
///
/// ★ Was 150 ms, on the reasoning that it bounded a frame's worst case. It did — and it also capped
/// THROUGHPUT at one read per frame, because it is longer than [`POLL_BUDGET`]: the first read that
/// found no data blocked 150 ms, blew the budget, and ended the poll. Measured on hardware, an 86 KB
/// page took 19 polls at 208 ms each — 8 KB/s on a 30 Mbps link, with the network never once the
/// limiting factor.
///
/// Short is better on both counts. A poll that finds nothing now costs 20 ms instead of 150, so the
/// window repaints *more* often, and a poll that finds data keeps draining until the socket is
/// actually empty (see `Stage::Body`).
const SOCKET_TIMEOUT: Duration = Duration::from_millis(20);
/// The budget for a single socket operation *while the TLS handshake is still running*.
///
/// Much longer than [`SOCKET_TIMEOUT`], and the reason is a real failure: at 150 ms a handshake
/// against a distant server never completed. A handshake is not like a body — a body arrives in
/// pieces and every piece is progress you keep, whereas a handshake flight is only useful once it is
/// complete, so a deadline that keeps cutting it off makes no progress to keep. It is also bounded
/// (one or two round trips), which a body is not, so spending a whole second inside one poll costs
/// at most one visibly dropped frame instead of an unbounded freeze.
const HANDSHAKE_TIMEOUT: Duration = Duration::from_millis(1000);
/// How much wall time one `poll` may spend before handing the frame back. Larger means fewer
/// repaints but better throughput.
const POLL_BUDGET: Duration = Duration::from_millis(120);
/// Whole-request deadline, across every redirect.
const TOTAL_DEADLINE: Duration = Duration::from_secs(45);
/// How many of a name's addresses to try before giving up.
///
/// A large site publishes several A records and some are routinely unreachable from any given
/// network — so the first one is a coin flip, and a client that cannot walk past it fails where
/// every browser succeeds. Bounded because each attempt costs the kernel's 10 s connect deadline,
/// and four is already past the point where the address is plausibly the problem.
const MAX_ADDR_ATTEMPTS: usize = 3;
/// How long a peer may stay completely silent after we have written to it.
///
/// Distinct from every other deadline here because it detects a distinct failure: a host that
/// completes a TCP handshake and then sends **nothing at all**. Measured on hardware against one of
/// Google's frontends — connect succeeded, a 238-byte ClientHello went out, and 43 consecutive
/// reads returned zero bytes until the 45 s whole-request deadline expired.
///
/// A TLS ServerHello arrives in one round trip. Zero bytes after ten seconds is not a slow server,
/// it is a dead path — a blackholed route, or a middlebox that accepts the connection and discards
/// what follows. The useful response is the one a failed connect already gets: abandon this address
/// and try the next, rather than spending the entire request budget on silence.
///
/// Armed only while NOTHING has arrived. A server that has sent one byte is talking to us, and is
/// governed by `TOTAL_DEADLINE` from then on.
const SILENT_PEER_TIMEOUT: Duration = Duration::from_secs(10);

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

    // ── Instrumentation ─────────────────────────────────────────────────────────────────────────
    // A fetch that stalls looks exactly like a fetch that is slow, and this machine has no serial
    // console — the `trace!` lines in `http.rs` are written and never read. So the counters that
    // distinguish the two have to be able to reach the screen. See `Fetch::diagnostic`.
    /// The address DNS gave us, kept past the connect so a failure can name what it dialled.
    ///
    /// Without it, "connect timed out" cannot be told apart from "connect timed out *to the wrong
    /// address*" — and a resolver that returns a plausible-looking wrong address is exactly the
    /// failure that looks like a network problem and is not one.
    addr: Option<std::net::SocketAddr>,
    /// How many times `poll` has been called.
    polls: usize,
    /// Plaintext bytes handed up by the transport. NOT socket bytes — for TLS this stays at zero
    /// for the whole handshake by design, which is why `sock` exists beside it.
    bytes_in: usize,
    /// The raw socket's counters. This is the one that answers "is the peer replying at all".
    sock: Option<std::sync::Arc<crate::http::SockStats>>,
    /// When the current connection was established, for [`SILENT_PEER_TIMEOUT`].
    connected_at: Option<Instant>,
    /// This request is running on a connection taken from the idle slot.
    ///
    /// ★ Kept because a reused connection can fail in a way a fresh one cannot: the server may have
    /// closed it while it sat idle, and we only find out when the read after our request returns
    /// EOF having produced nothing. That is not a real failure — it is the expected race in every
    /// keep-alive implementation — and the answer is to reconnect and send it again ONCE. A GET is
    /// idempotent, so replaying it is safe.
    on_reused: bool,
    /// Whether that retry has already been spent.
    reused_retry_done: bool,
    /// Every address the name resolved to, and which one is being tried.
    ///
    /// ★ Held here rather than re-resolved on each failure. Going back to DNS to find the next
    /// candidate costs a round trip we do not need — the whole list arrived in the first answer —
    /// and, worse, a transient resolver failure on the retry then reported "cannot resolve" for a
    /// name that had *already resolved*, burying the connect failure that was the real cause.
    candidates: Vec<std::net::SocketAddr>,
    candidate_idx: usize,
    /// The last error a read produced, kept as a string because `io::Error` is not `Clone`.
    last_err: Option<String>,
    /// How many times a failed connect has sent us back to re-resolve. Bounded, so a host that is
    /// genuinely unreachable fails instead of looping.
    connect_retries: u8,
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
            addr: None,
            sock: None,
            connected_at: None,
            on_reused: false,
            reused_retry_done: false,
            candidates: Vec::new(),
            candidate_idx: 0,
            polls: 0,
            bytes_in: 0,
            last_err: None,
            connect_retries: 0,
        }
    }

    /// One line saying what this fetch is actually doing, for a UI to put on screen.
    ///
    /// Deliberately dense and unpolished: it is a diagnostic, and the thing it has to do is let
    /// someone photograph a stalled window and know from the photograph which of three failures it
    /// is — nothing arriving, arriving but the handshake not completing, or completing but the
    /// response never framing.
    pub fn diagnostic(&self) -> String {
        let stage = match self.stage {
            Stage::Resolve => "resolve",
            Stage::Connect(_) => "connect",
            Stage::Head => "head",
            Stage::Body => "body",
            Stage::Finished => "done",
        };
        let tls = match self.transport.as_ref().and_then(|t| t.tls_progress()) {
            Some((hs, wr, ww)) => format!(
                " tls[hs={} r={} w={}]",
                if hs { "YES" } else { "no" },
                wr as u8,
                ww as u8
            ),
            None => String::new(),
        };
        let addr = match self.addr {
            Some(a) => format!(" addr={a}"),
            None => " addr=UNRESOLVED".to_string(),
        };
        let sock = match self.sock.as_ref() {
            Some(s) => format!(" {}", s.summary()),
            None => String::new(),
        };
        format!(
            "{stage}{addr} polls={} plain={}B body={}B{tls}{sock} last={}",
            self.polls,
            self.bytes_in,
            self.body.len(),
            self.last_err.as_deref().unwrap_or("-")
        )
    }

    /// The URL currently being fetched, which is not the one asked for once a redirect is taken.
    pub fn url(&self) -> &Url {
        &self.url
    }

    /// Do one frame's worth of work.
    pub fn poll(&mut self) -> Progress {
        self.polls += 1;
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
                let addrs = resolve_candidates(&self.url)?;
                let first = addrs[0];
                self.candidates = addrs;
                self.candidate_idx = 0;
                self.addr = Some(first);
                self.enter(Stage::Connect(first));
                Ok(Progress::Connecting(Phase::Connecting))
            }
            Stage::Connect(addr) => {
                // A kept connection skips DNS, the TCP handshake AND the TLS handshake — which on
                // this machine is the dominant cost of following a link, since the certificate is
                // verified in software.
                if let Some(mut transport) = crate::http::take_idle(&self.url) {
                    transport.set_timeout(SOCKET_TIMEOUT)?;
                    transport.queue_request(&crate::http::request_line(&self.url, true))?;
                    self.sock = Some(transport.stats());
                    self.connected_at = Some(Instant::now());
                    self.transport = Some(transport);
                    self.on_reused = true;
                    self.enter(Stage::Head);
                    return Ok(Progress::Connecting(Phase::Headers));
                }

                if self.announcing() {
                    return Ok(Progress::Connecting(Phase::Connecting));
                }
                let mut transport = match crate::http::Transport::connect(&self.url, addr, &self.started)
                {
                    Ok(t) => t,
                    Err(e) => {
                        // A cached address that will not answer is worse than no cache: it turns
                        // one unlucky DNS reply into ten minutes of certain failure. Drop it and
                        // ask again — a name with several A records (which any large site has) will
                        // usually hand back a different one.
                        // This address did not answer. That says nothing about the OTHERS the
                        // resolver gave us, so drop only this one and come back for the next —
                        // which is what every mainstream client does and why a phone succeeds on a
                        // network where a one-address client fails.
                        crate::dns::forget_addr(&self.url.host, addr.ip());
                        if self.advance_candidate() {
                            self.last_err =
                                Some(format!("{} refused; trying the next address", addr.ip()));
                            return Ok(Progress::Connecting(Phase::Connecting));
                        }
                        return Err(e);
                    }
                };
                // The handshake gets the long budget; the body gets the short one, set when we
                // enter `Stage::Body` below. A handshake flight cut off part-way leaves nothing
                // behind to resume from, so slicing it finely does not buy responsiveness, it just
                // prevents it finishing.
                transport.set_timeout(HANDSHAKE_TIMEOUT)?;
                transport.queue_request(&crate::http::request_line(&self.url, true))?;
                self.on_reused = false;
                self.sock = Some(transport.stats());
                self.connected_at = Some(Instant::now());
                self.transport = Some(transport);
                self.enter(Stage::Head);
                Ok(Progress::Connecting(self.head_phase()))
            }
            Stage::Head => {
                // A peer that took the connection and has since said nothing is a dead path. Move
                // on to the next address rather than spending the whole request budget on silence.
                if self.silent_too_long() {
                    let addr = self.addr;
                    if let Some(a) = addr {
                        crate::dns::forget_addr(&self.url.host, a.ip());
                    }
                    if self.advance_candidate() {
                        self.last_err = Some(format!(
                            "{} went silent; trying the next",
                            addr.map(|a| a.ip().to_string()).unwrap_or_default()
                        ));
                        return Ok(Progress::Connecting(Phase::Connecting));
                    }
                    return Err(Error::Connect {
                        addr: self.addr.map(|a| a.to_string()).unwrap_or_default(),
                        source: std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "accepted the connection then sent nothing",
                        ),
                    });
                }

                // EOF here is fatal and must be reported. Ignoring it left the fetch announcing
                // "Connecting" every frame against a closed socket until the 45 s whole-request
                // deadline — which reads as a hang, and hides the real fault.
                if self.fill_n()?.is_none() {
                    // The keep-alive race: the server closed this connection while it was idle, and
                    // said so only by hanging up on the request we just sent. Reconnect and send it
                    // once more. Bounded to a single retry so a server that closes every connection
                    // cannot loop us.
                    if self.on_reused && !self.reused_retry_done && self.bytes_in == 0 {
                        self.reused_retry_done = true;
                        self.on_reused = false;
                        self.transport = None;
                        self.sock = None;
                        self.connected_at = None;
                        self.pending.clear();
                        self.cursor = 0;
                        self.scanned = 0;
                        self.last_err = Some("kept connection was closed; reconnecting".into());
                        let addr = self.addr.unwrap_or(self.candidates[self.candidate_idx]);
                        self.enter(Stage::Connect(addr));
                        return Ok(Progress::Connecting(Phase::Connecting));
                    }
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
                // Headers are in, so the handshake is long done: drop back to the short budget so
                // the body streams at frame rate.
                if let Some(t) = self.transport.as_mut() {
                    t.set_timeout(SOCKET_TIMEOUT)?;
                }
                self.enter(Stage::Body);
                Ok(Progress::Receiving { got: 0, total: self.total })
            }
            Stage::Body => {
                let deadline = Instant::now();
                loop {
                    if self.decode_body()? {
                        return self.finish(true);
                    }
                    if deadline.elapsed() > POLL_BUDGET {
                        return Ok(Progress::Receiving {
                            got: self.body.len(),
                            total: self.total,
                        });
                    }
                    match self.fill_n()? {
                        // EOF with an unfinished body is not an error: a truncated page still
                        // renders, and `Connection: close` responses end exactly this way.
                        None => return self.finish(false),
                        // Nothing waiting *this instant*. Keep asking until the budget is gone
                        // rather than handing the frame back.
                        //
                        // ★ This used to return immediately, and that was measured: a 486 KB
                        // Wikipedia page took 336 reads of which 303 timed out, and roughly 11 of
                        // its 14.5 seconds were spent idle — ~20 ms of a 120 ms budget used, then a
                        // 16 ms frame sleep, repeated. An empty socket means no data arrived in the
                        // last 20 ms, NOT that none will arrive in the next 100.
                        //
                        // Safe only because `SOCKET_TIMEOUT` is short. It is the pairing that
                        // matters: at the old 150 ms this loop would blow the budget on one read.
                        Some(0) => {}
                        // Data arrived, and where there is one segment there are usually more:
                        // keep draining within the budget instead of surrendering the frame.
                        Some(_) => {}
                    }
                }
            }
            Stage::Finished => Ok(Progress::Connecting(Phase::Headers)),
        }
    }

    /// Move to the next resolved address, if there is one left within the attempt budget.
    ///
    /// Clears the per-connection state so the next attempt starts clean — a stale `sock` would make
    /// [`Self::silent_too_long`] judge the new connection by the old one's byte count.
    fn advance_candidate(&mut self) -> bool {
        self.candidate_idx += 1;
        if self.candidate_idx >= self.candidates.len()
            || self.candidate_idx >= MAX_ADDR_ATTEMPTS
        {
            return false;
        }
        let next = self.candidates[self.candidate_idx];
        self.addr = Some(next);
        self.transport = None;
        self.connected_at = None;
        self.sock = None;
        self.pending.clear();
        self.cursor = 0;
        self.scanned = 0;
        self.enter(Stage::Connect(next));
        true
    }

    /// True when the peer has sent nothing at all for longer than [`SILENT_PEER_TIMEOUT`].
    ///
    /// Counts RAW socket bytes, not plaintext: during a TLS handshake `plain` stays at zero even
    /// when the server is answering perfectly, so testing plaintext here would abandon healthy
    /// connections.
    fn silent_too_long(&self) -> bool {
        use core::sync::atomic::Ordering::Relaxed;
        let Some(since) = self.connected_at else { return false };
        let Some(sock) = self.sock.as_ref() else { return false };
        sock.read_bytes.load(Relaxed) == 0 && since.elapsed() > SILENT_PEER_TIMEOUT
    }

    /// Whether we are still shaking hands, or waiting on the reply.
    ///
    /// ★ Ask rustls, do not infer. This used to guess from `pending.is_empty()`, on the reasoning
    /// that "no bytes yet" and "handshaking" are the same state. They are not: once the handshake
    /// completes, `pending` is still empty until the first byte of the *reply* arrives, so a fetch
    /// that had finished its handshake and was waiting on the server kept reporting "Securing
    /// connection to…" — pointing every diagnosis at TLS when TLS was already done.
    fn head_phase(&self) -> Phase {
        let handshaking = self
            .transport
            .as_ref()
            .and_then(|t| t.tls_progress())
            .map(|(hs, _, _)| hs)
            .unwrap_or(false);
        if handshaking {
            Phase::Handshake
        } else {
            Phase::Headers
        }
    }

    /// Hand back the response, or follow a redirect by restarting against the new URL.
    /// Hand back the response.
    ///
    /// `framed` says the body ended where its framing said it would — `Content-Length` satisfied or
    /// the zero chunk seen — rather than because the peer hung up. Only a framed body leaves the
    /// connection in a known state, and only a known state may be reused.
    fn finish(&mut self, framed: bool) -> Result<Progress, Error> {
        let redirecting = matches!(self.status, 301 | 302 | 303 | 307 | 308);
        let location = self
            .headers
            .iter()
            .find(|(k, _)| k.eq_ignore_ascii_case("location"))
            .map(|(_, v)| v.clone());

        // Offer the connection back before anything drops it. A redirect to the same origin — which
        // `http://x` -> `https://www.x/` is not, but `/a` -> `/b` very much is — then costs nothing.
        if crate::http::response_is_reusable(&self.headers, framed) {
            if let Some(t) = self.transport.take() {
                crate::http::put_idle(&self.url, t);
            }
        }

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
        let headers = core::mem::take(&mut self.headers);
        // Inflate before handing the response over, so `Progress::Done` means the same thing here as
        // `http::get` does — a caller must never have to ask which of the two produced a body.
        let body = crate::http::decode_content_encoding(&headers, core::mem::take(&mut self.body))?;
        Ok(Progress::Done(Box::new(Response {
            status: self.status,
            headers,
            body,
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

    /// Read once from the socket, reporting how much arrived.
    ///
    /// `None` means the peer closed. `Some(0)` means nothing was available this time — which is a
    /// different answer from "the peer is gone", and the caller needs to tell them apart in order to
    /// know whether to keep draining or come back next frame. Collapsing both into a bool is what
    /// made a bulk transfer read exactly once per poll.
    fn fill_n(&mut self) -> Result<Option<usize>, Error> {
        let Some(transport) = self.transport.as_mut() else { return Ok(None) };
        let mut chunk = [0u8; 16 * 1024];
        let result = transport.read(&mut chunk);
        match result {
            Ok(0) => {
                self.last_err = Some("eof".into());
                Ok(None)
            }
            Ok(n) => {
                self.bytes_in += n;
                self.last_err = None;
                self.pending.extend_from_slice(&chunk[..n]);
                Ok(Some(n))
            }
            Err(e) if would_block(&e) => {
                // Not a failure: this is the frame's answer, and it is the expected one most of the
                // time. Recorded anyway, because "every read timed out" and "reads are succeeding"
                // are the two halves of the stall diagnosis.
                self.last_err = Some(format!("timeout({:?})", e.raw_os_error()));
                Ok(Some(0))
            }
            Err(e) if crate::http::is_clean_eof(&e) => {
                self.last_err = Some("clean-eof".into());
                Ok(None)
            }
            Err(e) => {
                self.last_err = Some(format!("{:?}/{:?}", e.kind(), e.raw_os_error()));
                Err(e.into())
            }
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
    // The blocking path in `http.rs` takes one address; the stepped path walks the whole list.
    resolve_candidates(url).map(|v| v[0])
}

fn resolve_candidates(url: &Url) -> Result<Vec<std::net::SocketAddr>, Error> {
    // The cache is checked first because the miss path is genuinely expensive: the kernel's resolver
    // busy-polls the network stack until an answer arrives or five seconds elapse, so a repeat
    // lookup does not merely cost a round trip, it costs a round trip of spinning. See `crate::dns`.
    //
    // The FIRST of the cached set, not the only one: a connect failure drops just that address (see
    // `Stage::Connect`), so the next attempt naturally advances to the next candidate.
    if let Some(hit) = crate::dns::lookup_all(&url.host, url.port) {
        if !hit.is_empty() {
            return Ok(hit);
        }
    }

    // An IP literal is not a name and must not be cached under one — the cache would then be full of
    // entries mapping an address to itself.
    if let Ok(ip) = url.host.parse::<std::net::IpAddr>() {
        return Ok(vec![std::net::SocketAddr::new(ip, url.port)]);
    }

    let addrs = crate::dns::resolve_all(&url.host);
    if addrs.is_empty() {
        return Err(Error::Dns { host: url.host.clone() });
    }
    let out = addrs.iter().map(|ip| std::net::SocketAddr::new(*ip, url.port)).collect();
    crate::dns::remember_all(&url.host, addrs);
    Ok(out)
}

fn would_block(e: &std::io::Error) -> bool {
    const ETIMEDOUT: i32 = 110;
    const EAGAIN: i32 = 11;
    // ★ EINTR belongs here, and it is new: the kernel's socket loops could not be interrupted at
    // all until they learned to check for a pending signal. Now that they can, a signal arriving
    // mid-read surfaces as `Interrupted` — which is NOT a failure, it is "ask again", and treating
    // it as fatal would turn any signal into a broken page load. Retrying on `Interrupted` is the
    // convention every `Read` implementation follows for exactly this reason.
    const EINTR: i32 = 4;
    matches!(
        e.kind(),
        std::io::ErrorKind::TimedOut | std::io::ErrorKind::WouldBlock | std::io::ErrorKind::Interrupted
    ) || matches!(e.raw_os_error(), Some(ETIMEDOUT) | Some(EAGAIN) | Some(EINTR))
}
