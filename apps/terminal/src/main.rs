// Nyx Terminal — Workstream D (D3): ported from no_std to a `target_os = "nyx"` **std** binary.
//
// This is the same terminal UI as before (matrix-green shell over nyx-gui), now built on real std so
// it can host the pluggable compiler registry in D4 (which needs std::fs + the std-only
// `nyx_toolchains` crate). The port is the exact 4-item swap D2 proved out: the no_std app's own
// `_start`/.text.entry + #[global_allocator] + #[panic_handler] are gone — std owns the heap, the
// entry, and panics — and `alloc::` becomes `std::`. nyx-gui/nyx-api link in UNCHANGED.
//
// Behaviour is intentionally identical to the no_std terminal for this step; the `compile`/`toolchains`
// commands land in D4 so this boot test cleanly answers "did the std port work".
//
// `restricted_std` is rustc's required opt-in for a std built for an out-of-tree target it doesn't
// recognise; the nyx PAL is injected into rust-src by vendor/nyx-std/apply.py.
#![feature(restricted_std)]

// ELF entry — identical contract to apps/stdgui, apps/qcstudio and tests/stdhello: the kernel hands
// us a SysV stack [argc][argv..][NULL][envp..][NULL][auxv..] with RSP % 16 == 8. Install the main
// thread's native (fs:-relative) TLS block BEFORE anything touches a #[thread_local], then forward
// argc/argv to the rustc-generated C `main` and exit_group(231) with its return value.
core::arch::global_asm!(
    ".global _start",
    "_start:",
    "mov rbx, rsp",         // stash entry RSP (callee-saved across the call)
    "mov rdi, rsp",         // arg0 = &argc (the SysV info block) for TLS auxv walk
    "and rsp, -16",
    "call __nyx_setup_main_tls",
    "mov rdi, [rbx]",       // argc
    "lea rsi, [rbx + 8]",   // argv
    "and rsp, -16",
    "call main",            // rustc-generated `int main(int, char**)` -> std lang_start
    "mov edi, eax",         // propagate exit code
    "mov eax, 231",         // SYS_exit_group
    "syscall",
    "1:",
    "hlt",
    "jmp 1b",
);

use nyx_api::*;
use nyx_gui::app::NyxApp;
use nyx_gui::canvas::Canvas;
// D4: the pluggable compiler registry — dispatches a source file to a language backend by extension.
use nyx_toolchains::{Artifact, Registry};

// D4: bundled by Build.sh into the Terminal app dir, so `compile` with no argument has something to
// chew on out of the box (a Bell-pair .ql — same sample qcstudio uses).
const DEFAULT_SAMPLE: &str = "/mnt/nvme/apps/Terminal.nyx/sample.ql";

// ── Meridian, step 11: the terminal interior ────────────────────────────────────────────────────
//
// From the design's `.term` rule (`design/ver3.0/parts/03-components.html:201`):
//
//     .term { padding: 24px 26px; font-family: "JetBrains Mono"; font-size: 13px;
//             line-height: 22px; color: var(--fg-2); font-weight: 400; }
//     .dark .term { background: #0B0C0E; }
//     .term .p { color: var(--fg-4); }   /* the prompt sigil */
//     .term .o { color: var(--fg); }     /* what you typed, echoed */
//     .term .cur { width: 7px; height: 15px; background: var(--accent); }
//
// The old green-on-near-black (`0xFF00FF66`) is gone. It was the one surface in the OS still
// claiming a "matrix terminal" identity that Meridian does not have anywhere else.
const TERM_PX: usize = 13;
const TERM_LINE_H: usize = 22;
const PAD_X: usize = 26;
const PAD_Y: usize = 24;
const CUR_W: usize = 7;
const CUR_H: usize = 15;

/// Which theme the shell last told us about. Read by the painter, written by the message pump; an
/// app is single-threaded, so a plain atomic is the whole synchronisation story.
///
/// Meridian step 13 added `MSG_THEME_CHANGED` / `NyxApp::on_theme` precisely so an app could stop
/// guessing, and the terminal was the last one still hardcoding dark — for no better reason than
/// that it was ported before the message existed.
static DARK: core::sync::atomic::AtomicBool = core::sync::atomic::AtomicBool::new(true);

fn theme() -> nyx_meridian::tokens::Theme {
    if DARK.load(core::sync::atomic::Ordering::Relaxed) {
        nyx_meridian::tokens::Theme::dark()
    } else {
        nyx_meridian::tokens::Theme::light()
    }
}

/// The console's own ground: one step below `--surface`, so it reads as a recessed well rather than
/// another panel.
///
/// This used to be a hardcoded `0xFF0B_0C0E` "taken literally from the CSS" — which is byte for byte
/// `Theme::dark().sunken`, so it was the same colour written down twice with only one of them able
/// to follow a theme. It is the token now, which is also what gives the light theme its ground for
/// free instead of needing a second literal invented to match.
fn term_bg() -> u32 {
    theme().sunken
}

/// JetBrains Mono's slot in `nyx_gui::font`'s registry.
const MONO: usize = nyx_meridian::font::SLOT_MONO;

/// How much of a page goes into the scrollback.
///
/// This was 8000 — about two screens — because the scrollback was one `String` that the draw path
/// re-walked and re-allocated every frame, so a big page really did make the terminal unusable. Now
/// that the scrollback is lines and the wrap is cached, the per-frame cost is proportional to the
/// ~15 lines actually on screen and not to the history, so a whole page is affordable and the
/// truncation that used to be a necessity is just an amputation.
///
/// It is still bounded, because `MAX_BODY` in `libs/net` allows an 8 MB response and a machine with
/// no OOM killer should not try to lay all of that out. 400 000 characters is past the end of
/// essentially every real document.
const MAX_PAGE_CHARS: usize = 400_000;

/// Truncate on a CHARACTER boundary. `&s[..n]` panics mid-UTF-8, and page text is full of
/// multi-byte punctuation (curly quotes, em dashes) — so a byte cut is not a rare edge case here.
fn clip(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        None => s.to_string(),
        Some((cut, _)) => format!("{}\n… [{} more bytes not shown]\n", &s[..cut], s.len() - cut),
    }
}

/// Syscall 550: shift CLOCK_REALTIME by a signed number of seconds.
const SYS_SET_REALTIME_OFFSET: u64 = 550;
/// Syscall 551: leave a one-byte breadcrumb in CMOS that survives a freeze and the power cycle.
const SYS_DEBUG_MARK: u64 = 551;
/// Unix time at which this image was built, the floor for any clock `time sync` will accept.
///
/// Derived from `NYX_BUILD_STAMP` at compile time when it parses, with a conservative fallback of
/// 2025-01-01 — the same date `date` already uses to decide a clock is implausibly old.
const BUILD_UNIX: i64 = match option_env!("NYX_BUILD_UNIX") {
    Some(s) => match konst_parse_i64(s) {
        Some(v) => v,
        None => 1_735_689_600,
    },
    None => 1_735_689_600,
};

/// How far past the build stamp a reported time may be before it is refused. Twenty years: long
/// enough that an old image still syncs, short enough to catch a clock pushed far forward.
const MAX_CLOCK_AHEAD_SECS: i64 = 20 * 365 * 24 * 3600;

/// `str::parse` is not const, and this needs to run in a `const` initialiser.
const fn konst_parse_i64(s: &str) -> Option<i64> {
    let b = s.as_bytes();
    if b.is_empty() {
        return None;
    }
    let mut i = 0;
    let mut acc: i64 = 0;
    while i < b.len() {
        let d = b[i];
        if d < b'0' || d > b'9' {
            return None;
        }
        acc = acc * 10 + (d - b'0') as i64;
        i += 1;
    }
    Some(acc)
}

/// Syscall 552: write the battery-backed RTC, so a clock fix survives a reboot.
const SYS_SET_RTC: u64 = 552;
/// Syscall 553/554: read/persist the DISPLAY timezone, in minutes east of UTC.
const SYS_GET_TZ: u64 = 553;
const SYS_SET_TZ: u64 = 554;

/// Minutes east of UTC, for display only.
fn tz_offset_min() -> i64 {
    syscall(SYS_GET_TZ, 0, 0, 0, 0, 0, 0) as i64
}

/// `+05:30` / `-08:00` / `+00:00` — how an offset is written everywhere else (ISO 8601, RFC 3339).
fn fmt_offset(min: i64) -> String {
    let sign = if min < 0 { '-' } else { '+' };
    let a = min.abs();
    format!("{sign}{:02}:{:02}", a / 60, a % 60)
}

/// Parse `+5:30`, `-8`, `+05:45`, `0`. Returns minutes east of UTC.
///
/// Minutes are not optional decoration: +05:30 (India), +05:45 (Nepal) and +12:45 (Chatham) are all
/// real, so an hours-only parser is wrong for a large part of the world.
fn parse_offset(s: &str) -> Option<i64> {
    let s = s.trim();
    let (neg, rest) = match s.strip_prefix('-') {
        Some(r) => (true, r),
        None => (false, s.strip_prefix('+').unwrap_or(s)),
    };
    let (h, m) = match rest.split_once(':') {
        Some((h, m)) => (h.parse::<i64>().ok()?, m.parse::<i64>().ok()?),
        None => (rest.parse::<i64>().ok()?, 0),
    };
    if !(0..=14).contains(&h) || !(0..60).contains(&m) {
        return None;
    }
    let total = h * 60 + m;
    Some(if neg { -total } else { total })
}

/// Record "we got this far" somewhere a hard freeze cannot erase. `0` clears it.
fn mark(step: u8) {
    syscall(SYS_DEBUG_MARK, step as u64, 0, 0, 0, 0, 0);
}

/// The RTC reading with no correction applied, so `time sync` can compute an offset that is not
/// relative to a previous offset — otherwise a second sync doubles the first one.
fn raw_unix_now() -> i64 {
    let t = sys_get_rtc();
    days_from_civil(t.year as i64, t.month as i64, t.day as i64) * 86_400
        + t.hour as i64 * 3600
        + t.min as i64 * 60
        + t.sec as i64
}

/// Days since 1970-01-01 for a civil date. Hinnant's algorithm, matching the kernel's copy in
/// `rtc_packed_to_unix` exactly — the two must agree or the computed offset is wrong by whatever
/// they disagree about.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;
    let doy = (153 * (if m > 2 { m - 3 } else { m + 9 }) + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146_097 + doe - 719_468
}

/// Parse an HTTP `Date:` header into Unix seconds.
///
/// Only IMF-fixdate — `Sun, 06 Nov 1994 08:49:37 GMT`. RFC 7231 requires servers to send exactly
/// this form; the two obsolete formats it also lists are for parsers that must accept anything, and
/// accepting them here would be code that never runs. Always GMT, so no zone handling.
fn parse_http_date(s: &str) -> Option<i64> {
    const MONTHS: [&str; 12] = ["Jan", "Feb", "Mar", "Apr", "May", "Jun",
                                "Jul", "Aug", "Sep", "Oct", "Nov", "Dec"];
    // "Sun, 06 Nov 1994 08:49:37 GMT" -> [Sun,] [06] [Nov] [1994] [08:49:37] [GMT]
    let f: Vec<&str> = s.trim().split_whitespace().collect();
    if f.len() < 5 {
        return None;
    }
    let day: i64 = f[1].parse().ok()?;
    let month = MONTHS.iter().position(|m| *m == f[2])? as i64 + 1;
    let year: i64 = f[3].parse().ok()?;
    let hms: Vec<&str> = f[4].split(':').collect();
    if hms.len() != 3 {
        return None;
    }
    let (h, mi, sec): (i64, i64, i64) =
        (hms[0].parse().ok()?, hms[1].parse().ok()?, hms[2].parse().ok()?);
    if !(1..=12).contains(&month) || !(1..=31).contains(&day) || h > 23 || mi > 59 || sec > 60 {
        return None;
    }
    Some(days_from_civil(year, month, day) * 86_400 + h * 3600 + mi * 60 + sec)
}

/// Unix seconds -> `YYYY-MM-DD HH:MM:SS UTC`.
///
/// Howard Hinnant's civil_from_days, the exact inverse of the `days_from_civil` the kernel uses in
/// `rtc_packed_to_unix`. Written out rather than pulled from a crate so that `date` depends on
/// nothing that could itself be the thing that is wrong.
fn fmt_unix(secs: i64) -> String {
    let days = secs.div_euclid(86_400);
    let tod = secs.rem_euclid(86_400);

    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = z - era * 146_097; // [0, 146096]
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365; // [0, 399]
    let y = yoe + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100); // [0, 365]
    let mp = (5 * doy + 2) / 153; // [0, 11], March-based
    let d = doy - (153 * mp + 2) / 5 + 1;
    let m = if mp < 10 { mp + 3 } else { mp - 9 };
    let y = if m <= 2 { y + 1 } else { y };

    format!(
        "{:04}-{:02}-{:02} {:02}:{:02}:{:02} UTC",
        y, m, d, tod / 3600, (tod % 3600) / 60, tod % 60
    )
}

/// The same, for a single line in a list, without the trailing note.
fn clip_line(s: &str, max: usize) -> String {
    match s.char_indices().nth(max) {
        None => s.to_string(),
        Some((cut, _)) => format!("{}…", &s[..cut]),
    }
}

/// Split one logical line into display lines of at most `cols` characters, breaking at spaces.
///
/// JetBrains Mono is fixed-pitch, so this is arithmetic on character counts rather than a per-glyph
/// advance lookup — `advance_px_face` takes a mutex per call, and this runs over the whole history.
///
/// Chunks by `chars()`, never by byte index: page text is full of curly quotes and em dashes, so a
/// byte cut is a routine panic here rather than an edge case (the same reason `clip` exists).
///
/// Two behaviours worth naming, both of which only matter now that the terminal renders prose:
///
/// * The break goes at the last space that fits, not at column `cols`. Chopping mid-word is fine for
///   a hex dump and unreadable for a paragraph. A word longer than the whole line — a URL, which is
///   most of what `links` prints — still hard-breaks, because the alternative is a line that runs
///   off the edge.
/// * Continuations are indented to hang under the text they continue, so a wrapped bullet stays
///   visibly one bullet instead of starting a new column of text at the margin.
fn wrap_logical_line(logical: &str, cols: usize, out: &mut Vec<String>) {
    if logical.is_empty() {
        out.push(String::new());
        return;
    }

    let chars: Vec<char> = logical.chars().collect();
    if chars.len() <= cols {
        out.push(logical.to_string());
        return;
    }

    // Never let the hanging indent eat so much of the line that there is no room left to wrap into.
    let indent = hanging_indent(&chars).min(cols / 2);
    let mut start = 0usize;
    let mut prefix = 0usize;

    while start < chars.len() {
        let budget = cols.saturating_sub(prefix).max(1);
        if chars.len() - start <= budget {
            out.push(indent_of(prefix) + &chars[start..].iter().collect::<String>());
            return;
        }

        // The break point: the last space within budget. `budget` itself is a legal break, since
        // breaking *at* a space consumes it rather than pushing a character past the edge.
        let window_end = start + budget;
        let brk = (start + 1..=window_end).rev().find(|&i| chars[i - 1] == ' ');

        let (cut, resume) = match brk {
            // A word longer than the line has no break point; take the hard cut rather than
            // letting the line run off the edge.
            None => (window_end, window_end),
            Some(i) => (i - 1, i),
        };
        out.push(indent_of(prefix) + &chars[start..cut].iter().collect::<String>());

        // Leading spaces on the continuation are the ones the break consumed; they are not content.
        start = resume;
        while start < chars.len() && chars[start] == ' ' {
            start += 1;
        }
        prefix = indent;
    }
}

fn indent_of(n: usize) -> String {
    " ".repeat(n)
}

/// How far a wrapped continuation should be indented to hang under its first line.
///
/// The leading whitespace, plus the width of a list bullet or quote marker if one is there — those
/// are the markers `nyx_htmltext` emits, and a continuation that starts under the bullet rather than
/// under the text reads as a second item.
fn hanging_indent(chars: &[char]) -> usize {
    let lead = chars.iter().take_while(|c| **c == ' ').count();
    let rest = &chars[lead..];
    let marker = if rest.starts_with(&['•', ' ']) || rest.starts_with(&['>', ' ']) {
        2
    } else {
        // "1. ", "12. " — an ordered-list marker of any width.
        let digits = rest.iter().take_while(|c| c.is_ascii_digit()).count();
        if digits > 0 && rest.get(digits) == Some(&'.') && rest.get(digits + 1) == Some(&' ') {
            digits + 2
        } else {
            0
        }
    };
    lead + marker
}

/// The scrollback, stored as lines rather than as one `String`.
///
/// It used to be a single `String` that the draw path re-split, re-walked and re-allocated into a
/// fresh `Vec<String>` on every frame — and that was described in a comment as safe because "the
/// history is capped at `MAX_PAGE_CHARS`". It was not: `MAX_PAGE_CHARS` caps *one page*, and nothing
/// capped the total. Ten pages left an 80 000-character history, `acpi log` adds 16 KB in a single
/// command, and the per-frame cost grew with all of it forever.
///
/// Two things follow from storing lines. The wrap can be cached against [`Scrollback::revision`],
/// because a frame that changed nothing can now prove it changed nothing. And the history can be
/// bounded, because there is something countable to bound.
///
/// The last element is always the line currently being written to; `push_str` starts a new one at
/// each `\n`. That keeps the append-only API (`push_str`, `push`, `clear`) the ~200 existing call
/// sites already use, so none of them had to change.
struct Scrollback {
    lines: Vec<String>,
    /// Bumped on every mutation. The draw path compares it to decide whether its wrap is still good.
    revision: u64,
}

/// How many logical lines of scrollback survive. Generous — trimming shifts everything above the
/// view, and a jump while you are reading is worse than the memory it saves — but finite, which is
/// the part that matters.
const MAX_SCROLLBACK_LINES: usize = 12000;

impl Scrollback {
    fn new(initial: &str) -> Scrollback {
        let mut s = Scrollback { lines: vec![String::new()], revision: 0 };
        s.push_str(initial);
        s
    }

    fn push_str(&mut self, s: &str) {
        if s.is_empty() {
            return;
        }
        for (i, part) in s.split('\n').enumerate() {
            if i > 0 {
                self.lines.push(String::new());
            }
            if !part.is_empty() {
                // `lines` is never empty: `new` seeds it and `clear` re-seeds it.
                self.lines.last_mut().unwrap().push_str(part);
            }
        }
        self.trim();
        self.revision = self.revision.wrapping_add(1);
    }

    fn push(&mut self, c: char) {
        if c == '\n' {
            self.lines.push(String::new());
            self.trim();
        } else {
            self.lines.last_mut().unwrap().push(c);
        }
        self.revision = self.revision.wrapping_add(1);
    }

    fn clear(&mut self) {
        self.lines.clear();
        self.lines.push(String::new());
        self.revision = self.revision.wrapping_add(1);
    }

    /// True when the open line has nothing on it — i.e. output is at the start of a fresh line.
    fn at_line_start(&self) -> bool {
        self.lines.last().map(|l| l.is_empty()).unwrap_or(true)
    }

    fn trim(&mut self) {
        if self.lines.len() > MAX_SCROLLBACK_LINES {
            // One `drain` rather than repeated `remove(0)`, which is quadratic in the overflow.
            let excess = self.lines.len() - MAX_SCROLLBACK_LINES;
            self.lines.drain(..excess);
        }
    }
}

/// A page, kept whole.
///
/// The whole point of holding it rather than only printing it: `find` needs something to search,
/// `reader` needs something to re-render without going back to the network, and `open <n>` needs the
/// links and the base to still be around after the user has scrolled past them.
struct LoadedPage {
    /// Where the body actually came from, AFTER redirects. Relative links resolve against this
    /// rather than against what was typed — an href on a page reached through a 301 is relative to
    /// where the page came FROM, not to where we asked.
    url: String,
    title: String,
    text: String,
    links: Vec<nyx_htmltext::Link>,
    /// `<base href>` if the document declared one. Overrides `url` for link resolution, which is
    /// what the HTML standard says and what the sites that use it depend on.
    base: Option<String>,
    /// The raw body, kept so `reader` can re-render without a second request.
    source: String,
    /// False for a body that was not markup — JSON, plain text — which is shown verbatim.
    is_html: bool,
}

/// Session state for browsing: where we have been, and where we are in it.
#[derive(Default)]
struct Browser {
    /// Visited URLs, oldest first. `back` and `forward` move `pos` within it.
    history: Vec<String>,
    /// Index into `history` of the page being shown. Meaningless when `history` is empty.
    pos: usize,
    page: Option<LoadedPage>,
    /// Whether to drop navigation chrome when rendering. Persists across pages, because a reader who
    /// wants the article on one page wants it on the next.
    reader: bool,
    /// The last `find` term and how far through the page it has got, so `next` has a meaning.
    find_term: String,
    find_from: usize,
}

impl Browser {
    /// The URL relative links on the current page resolve against.
    fn base_url(&self) -> Option<&str> {
        let page = self.page.as_ref()?;
        Some(page.base.as_deref().unwrap_or(page.url.as_str()))
    }

    /// Record a visit. Navigating after going back truncates the forward entries, exactly as a
    /// browser does — the forward history is a branch you left, not a queue.
    fn visit(&mut self, url: &str) {
        if !self.history.is_empty() && self.history.get(self.pos).map(String::as_str) == Some(url) {
            return; // a reload of the same page is not a new history entry
        }
        if !self.history.is_empty() {
            self.history.truncate(self.pos + 1);
        }
        self.history.push(url.to_string());
        self.pos = self.history.len() - 1;
    }
}

/// What a completed load should do to the history.
#[derive(Clone, Copy, PartialEq)]
enum Nav {
    /// A new destination: push it and drop any forward entries.
    Push,
    /// `back`, `forward` or `reload` — `pos` is already where it should be.
    Keep,
}

/// A fetch in flight.
///
/// `nyx_net::Fetch` has existed since the browser was written and had exactly one caller in the
/// whole repository: an example. The terminal called the blocking `nyx_net::get` instead, from
/// inside `on_key`, which is inside the window loop's IPC drain — so for the entire duration of a
/// page load the window did not repaint, did not blink its cursor, and did not process
/// `MSG_WINDOW_CLOSE`. It looked alive and could not be closed.
struct Loading {
    fetch: Box<nyx_net::Fetch>,
    /// What the user asked for, for the messages — `fetch.url()` moves as redirects are followed.
    requested: String,
    nav: Nav,
    started: std::time::Instant,
    /// The link's counters when this load began, so the report can be a DELTA.
    ///
    /// Cumulative counters cannot answer "how did THIS transfer go" — the interesting figures are
    /// frames per KB and bytes per second across one fetch, and both are differences.
    link_at_start: WifiRing,
}

struct TerminalApp {
    input_buffer: String,
    output_history: Scrollback,
    blink_timer: usize,
    cursor_visible: bool,

    /// Everything about the page being read.
    browser: Browser,
    /// The fetch in flight, pumped from `update` once a frame. `None` means nothing is loading.
    loading: Option<Loading>,
    /// A line drawn under the scrollback and above the prompt while something is happening.
    ///
    /// Deliberately NOT part of the scrollback. A progress line that lives in the history has to be
    /// the last line to be rewritable, which means nothing else may print while a page loads — so
    /// either the terminal locks up its own command line for the duration, or the progress report
    /// scrolls past sixty times a second. Keeping it outside the history costs one branch in the
    /// wrap and removes the whole problem.
    status_line: Option<String>,

    /// Commands entered, oldest first, walked with the up and down arrows.
    ///
    /// The arrows were previously pushed into the input buffer as literal U+E012/U+E013 glyphs, so
    /// pressing Up printed a box and retyping the last URL meant retyping the last URL. There is no
    /// Ctrl on this machine (`HandleControl::Ignore` in the kernel's keyboard), so the arrows are
    /// the only affordance available for this.
    cmd_history: Vec<String>,
    /// Where in `cmd_history` the arrows are. `== len` means "at the live line, not in history".
    cmd_pos: usize,
    /// The half-typed line the user was on before they started walking history, so that arrowing all
    /// the way back down returns it instead of leaving them on an empty prompt.
    cmd_stash: String,

    // ── Scrollback ──────────────────────────────────────────────────────────────────────────
    /// First visible display line. Counted in *wrapped* lines, not logical ones, so it maps
    /// straight onto what is on screen.
    scroll: usize,
    /// Follow new output. True until the user scrolls up, restored when they return to the
    /// bottom — the behaviour every terminal has, and the reason `acpi log` does not rip the view
    /// away while you are reading the middle of it.
    stick_bottom: bool,
    /// Wrapped-line count and column width from the last `draw`.
    ///
    /// `content_height` has no canvas to measure against, so the draw pass leaves its measurements
    /// here. One frame stale at worst, which is what `apps/notepad` does for the same reason.
    wrapped_len: usize,
    view_rows: usize,

    // ── Wrap cache ──────────────────────────────────────────────────────────────────────────────
    /// The scrollback split into display lines. Held across frames, because the wrap only changes
    /// when one of the three things below changes — and while you hold Page Down, none of them do.
    wrapped: Vec<String>,
    wrap_cols: usize,
    wrap_rev: u64,
    /// The live lines below the scrollback — the loading status, then the prompt — wrapped fresh
    /// every frame. Kept apart from `wrapped` so that animating them costs two lines of work
    /// rather than re-wrapping the whole history.
    tail: Vec<String>,
}

impl TerminalApp {
    fn new() -> Self {
        Self {
            input_buffer: String::new(),
            output_history: Scrollback::new("NyxOS v0.1 Shell\nType 'help' for commands.\n"),
            blink_timer: 0,
            cursor_visible: true,
            browser: Browser::default(),
            loading: None,
            status_line: None,
            cmd_history: Vec::new(),
            cmd_pos: 0,
            cmd_stash: String::new(),
            scroll: 0,
            stick_bottom: true,
            wrapped_len: 0,
            view_rows: 1,
            wrapped: Vec::new(),
            // `usize::MAX` cols and a revision the scrollback can never be at, so the first frame
            // always builds rather than trusting an empty cache.
            wrap_cols: usize::MAX,
            wrap_rev: u64::MAX,
            tail: Vec::new(),
        }
    }

    /// Bring [`Self::wrapped`] — the scrollback's wrap — up to date, if it has gone stale.
    ///
    /// It depends on exactly two things: the column count and the scrollback's revision. When
    /// neither has moved, the previous wrap is still correct and rebuilding it is pure waste. That
    /// waste used to be the terminal's dominant cost after a page load: a fresh `Vec` of
    /// freshly-allocated `String`s, one per display line, over the whole history, to then draw the
    /// ~15 of them that fit on screen.
    ///
    /// ★ The input line and the status line are deliberately NOT part of this, and are wrapped
    /// separately by [`Self::rebuild_tail`]. They are the two things on screen that change most
    /// often — the input on every keystroke, the status on every frame of a load, because it counts
    /// milliseconds — and folding them in here would mean re-wrapping twelve thousand lines of
    /// history sixty times a second to animate one of them. Splitting the cache at the boundary
    /// between "rarely changes and is enormous" and "changes constantly and is two lines" is the
    /// whole trick.
    fn rewrap(&mut self, cols: usize) {
        let cols = cols.max(1);
        if self.wrap_cols == cols && self.wrap_rev == self.output_history.revision {
            return;
        }

        // Taken out and put back so the wrapper can borrow the scrollback while filling it, and so
        // the `Vec`'s allocation (and each line's) survives from frame to frame.
        let mut out = core::mem::take(&mut self.wrapped);
        out.clear();
        for logical in self.output_history.lines.iter() {
            wrap_logical_line(logical, cols, &mut out);
        }

        self.wrapped = out;
        self.wrap_cols = cols;
        self.wrap_rev = self.output_history.revision;
    }

    /// Wrap the two live lines that sit under the scrollback: the loading status, then the prompt.
    ///
    /// Rebuilt unconditionally every frame, which is free — it is at most a few display lines.
    fn rebuild_tail(&mut self, cols: usize) {
        let cols = cols.max(1);
        let mut tail = core::mem::take(&mut self.tail);
        tail.clear();
        if let Some(status) = self.status_line.as_deref() {
            // Split on newlines first: `wrap_logical_line` takes ONE logical line and would
            // otherwise render an embedded '\n' as a glyph. The status is multi-line whenever it is
            // carrying the loader's diagnostic.
            for line in status.split('\n') {
                wrap_logical_line(line, cols, &mut tail);
            }
        }
        wrap_logical_line(&format!("N> {}", self.input_buffer), cols, &mut tail);
        self.tail = tail;
    }

    /// The display line at `idx` across the scrollback and the live tail, as one sequence.
    fn display_line(&self, idx: usize) -> &str {
        match self.wrapped.get(idx) {
            Some(l) => l.as_str(),
            None => self.tail.get(idx - self.wrapped.len()).map(String::as_str).unwrap_or(""),
        }
    }

    /// Largest first-visible line that still fills the viewport.
    fn max_scroll(&self) -> usize {
        self.wrapped_len.saturating_sub(self.view_rows)
    }

    /// Move the view and decide whether to keep following new output.
    ///
    /// Re-sticking on arrival at the bottom is deliberate: scrolling up to read then flicking back
    /// down should resume live output without a separate gesture.
    fn set_scroll(&mut self, want: usize) {
        let max = self.max_scroll();
        self.scroll = want.min(max);
        self.stick_bottom = self.scroll >= max;
    }

    /// One line describing the Wi-Fi link, including the DNS server from its DHCP lease.
    ///
    /// The kernel only routes sockets to the Wi-Fi stack once `init_wifi_iface` has published an
    /// interface, which needs a DHCP lease — so "connected but no IP" and "not connected" both mean
    /// traffic falls back to the wired NIC, and on this laptop that goes nowhere.
    fn link_summary(&self) -> String {
        match sys_wifi_status() {
            None => "wifi: no adapter\n".to_string(),
            Some(st) => {
                let state = match st.state {
                    WIFI_IDLE => "idle (not connected)",
                    WIFI_CONNECTING => "connecting",
                    WIFI_CONNECTED => "connected",
                    WIFI_AUTH_FAILED => "auth failed",
                    WIFI_NO_LEASE => "associated but NO DHCP LEASE",
                    WIFI_HW_FAILED => "hardware failure",
                    WIFI_RADIO_OFF => "radio OFF",
                    _ => "unknown",
                };
                // The RX parser's counters go on the same line as the lease because they answer the
                // question the lease cannot: whether frames are arriving and being discarded inside
                // the driver. `seen` climbing while `passed` does not is a driver bug wearing the
                // costume of a dead server.
                // `group` is broadcast/multicast we can never decrypt (GTK, KeyID 1) and never
                // asked for. `ours` is individually-addressed traffic going missing — the only
                // figure that can explain a broken TCP transfer, and the one a combined count hid.
                let (seen, passed, group, nosnap) = sys_wifi_rx_counters();
                let ours = nosnap.saturating_sub(group);
                format!(
                    "wifi: {} ssid={:?} ip={}.{}.{}.{} gw={}.{}.{}.{} dns={}.{}.{}.{}\n\
                     rx: seen={} passed={} dropped={} (group={} OURS={})\n",
                    state, st.name(),
                    st.ip[0], st.ip[1], st.ip[2], st.ip[3],
                    st.router[0], st.router[1], st.router[2], st.router[3],
                    st.dns[0], st.dns[1], st.dns[2], st.dns[3],
                    seen, passed, seen.saturating_sub(passed), group, ours,
                ) + &{
                    // The hardware side. `handed` is what the ring gave the parser; `seen` is what
                    // the parser recognised. And read_ptr vs closed is the comparison the whole RX
                    // path turns on — they are tested for inequality while living in different
                    // moduli, so read_ptr climbing past 4095 means every poll claims a frame forever.
                    let r = sys_wifi_rx_ring();
                    format!(
                        "ring: read_ptr={} closed={} handed={} badsig={} runaway={}{}\n\
                         traffic: tx={} frames/{} KB   rx={} KB   tx_dropped={}\n\
                         parse: desc={} scan={} undecrypted={} (descsz={} hdr claimed={} true={})\n\
                         {}",
                        r.read_ptr, r.closed, r.handed, r.badsig, r.runaway,
                        if r.read_ptr > 0x0FFF { "   << read_ptr PAST THE RING" } else { "" },
                        r.tx_frames, r.tx_bytes / 1024, r.rx_bytes / 1024, r.tx_nospace,
                        r.desc_hit, r.scan_hit, r.undecrypted,
                        r.desc_size, r.claimed_hdr, r.true_hdr,
                        // ★ The lease clock. Nothing renews it yet (audit M1), so the only defence
                        // against a lease quietly expiring is saying so before it does — a link
                        // that stops routing for no visible reason is the least debuggable failure
                        // this machine can produce.
                        match r.lease_secs {
                            0 => String::new(),
                            n if n < 120 => {
                                format!("lease: {n} s left  ** EXPIRING — `wifi join` to renew **\n")
                            }
                            n => format!("lease: {} min left\n", n / 60),
                        },
                    )
                }
            }
        }
    }

    /// `wifi …` — the whole radio, from a prompt.
    ///
    /// ★ Meridian step 20 retires `apps/wifi`, the standalone picker, and with it the only graphical
    /// way to *join* a network that existed outside the shell. The Entity's drill-down replaces it
    /// inside Meridian; this replaces it everywhere else. That matters more than it sounds: on a
    /// laptop whose only working link is the radio, "the desktop did not come up" and "the machine
    /// cannot get online" must not be the same sentence.
    ///
    /// Unlike the shell, this app may block. It is not the window server — a ten-second join costs
    /// this window's responsiveness and nothing else — so it calls the kernel directly rather than
    /// going through the agent. The shell's reasons for not doing that are in `nyx_api::WifiOp`.
    fn do_wifi(&mut self, arg: &str) {
        let arg = arg.trim();
        if arg.is_empty() || arg == "status" {
            let s = self.link_summary();
            self.output_history.push_str(&s);
            let mut buf = [0u8; 160];
            match wifi_load_network(&mut buf) {
                Some((slen, _)) => {
                    let name = core::str::from_utf8(&buf[..slen]).unwrap_or("?").to_string();
                    self.output_history
                        .push_str(&format!("remembered: {:?} (rejoined at boot)\n", name));
                }
                None => self.output_history.push_str("remembered: none\n"),
            }
            return;
        }

        if arg == "list" || arg == "scan" {
            // `scan` re-sweeps and BLOCKS ~7 s; `list` just reads the last sweep. Both then print
            // the same table, because "what did that just find" is the only reason to run either.
            if arg == "scan" {
                self.output_history
                    .push_str("Scanning every channel (about seven seconds)...\n");
                sys_wifi_scan();
            }
            let mut buf = [WifiNetwork::default(); 48];
            let n = sys_wifi_list(&mut buf);
            if n == 0 {
                self.output_history.push_str("no networks in range\n");
                return;
            }
            let mut nets: Vec<WifiNetwork> = buf[..n].to_vec();
            nets.sort_by(|a, b| a.name().cmp(b.name()));
            for e in &nets {
                let band = if e.band == 1 { "2.4GHz" } else { "5GHz" };
                // Named the same way the drill-down names them, because a person reading both
                // should not have to work out that "Mixed mode" and "TKIP" are the same network.
                let sec = if e.needs_tkip() {
                    "WPA2/TKIP mixed - UNJOINABLE"
                } else if e.is_legacy_security() {
                    "WEP/WPA1 - UNJOINABLE"
                } else if e.is_secure() {
                    "WPA2"
                } else {
                    "open"
                };
                self.output_history.push_str(&format!(
                    "{}{:<24} {:>7} ch{:<4} {}\n",
                    if e.is_current() { "* " } else { "  " },
                    e.name(), band, e.channel, sec,
                ));
            }
            return;
        }

        // `wifi rx` — the frames the driver threw away.
        //
        // Every dropped frame on this link is a NOSNAP, and the counters cannot say why the LLC/SNAP
        // scan missed. These are the bytes it scanned. `fc1 & 0x40` set means the frame is marked
        // Protected — if the firmware did not decrypt it in place, there is no SNAP to find at any
        // offset. `fc1 & 0x80` is the Order bit: a QoS frame with HT Control carries 4 extra header
        // bytes that `hlen` does not account for.
        if arg == "rx" {
            let mut buf = [0u8; 160];
            let n = sys_wifi_nosnap_dump(&mut buf) as usize;
            if n == 0 {
                self.output_history
                    .push_str("No dropped frames captured (nothing has failed the SNAP scan yet).\n");
                return;
            }
            self.output_history.push_str("Frames dropped by the RX parser (no LLC/SNAP found):\n");
            for i in 0..n.min(4) {
                let r = &buf[i * 40..i * 40 + 40];
                let mpdu = r[4] as u16 | ((r[5] as u16) << 8);
                self.output_history.push_str(&format!(
                    "  [{}] fc0={:02x} fc1={:02x} hlen={} ccmp={} len={}{}{}\n",
                    i + 1, r[0], r[1], r[2], r[3], mpdu,
                    if r[1] & 0x40 != 0 { " PROTECTED" } else { "" },
                    if r[1] & 0x80 != 0 { " ORDER(+4 HT ctl)" } else { "" },
                ));
                let mut hex = String::from("      ");
                for (k, b) in r[6..40].iter().enumerate() {
                    hex.push_str(&format!("{:02x} ", b));
                    if k == 15 {
                        hex.push_str("\n      ");
                    }
                }
                self.output_history.push_str(&hex);
                self.output_history.push('\n');
            }
            self.output_history
                .push_str("  (looking for aa aa 03 00 00 00 within 20 bytes of the header end)\n");
            return;
        }

        if arg == "on" || arg == "off" {
            let want = arg == "on";
            let got = sys_wifi_set_radio(want) != WIFI_RADIO_OFF;
            // Believe the return value, not the request: the kernel refuses while another operation
            // owns the radio and hands back the state unchanged.
            self.output_history.push_str(if got != want {
                "the radio is busy with another operation - try again\n"
            } else if want {
                "radio on (nothing rejoined; use `wifi join`)\n"
            } else {
                "radio off\n"
            });
            return;
        }

        if arg == "leave" || arg == "disconnect" {
            sys_wifi_disconnect();
            self.output_history.push_str("disconnected\n");
            return;
        }

        if arg == "forget" {
            self.output_history.push_str(if wifi_forget_network() {
                "forgotten - this machine will not reconnect on its own\n"
            } else {
                "could not rewrite the saved-network file\n"
            });
            return;
        }

        if let Some(rest) = arg.strip_prefix("join ") {
            // `join <ssid> [passphrase]`. The SSID is everything up to the LAST space when there is
            // a passphrase, because an SSID may contain spaces and a WPA2 passphrase may not be
            // shorter than eight characters — so the split is decidable without quoting rules.
            let rest = rest.trim();
            let (ssid, psk) = match rest.rfind(' ') {
                Some(i) if rest.len() - i - 1 >= 8 => (&rest[..i], &rest[i + 1..]),
                _ => (rest, ""),
            };
            if ssid.is_empty() {
                self.output_history.push_str("usage: wifi join <ssid> [passphrase]\n");
                return;
            }
            // ⚠️ Said out loud, every time. The passphrase was typed in the clear and is now in the
            // scrollback, and `wifi.conf` is plain text on an unencrypted filesystem.
            self.output_history.push_str(&format!(
                "Joining {:?}. This blocks for ten seconds or so.\n\
                 The passphrase is in this window's scrollback and, once saved, in\n\
                 /mnt/nvme/wifi.conf in plain text. Nyx has no disk encryption.\n",
                ssid
            ));
            let rc = sys_wifi_connect(ssid, psk);
            self.output_history.push_str(match rc {
                WIFI_CONNECTED => "connected\n",
                WIFI_AUTH_FAILED => "the password was rejected\n",
                WIFI_NO_LEASE => "joined, but the router never offered an address\n",
                WIFI_HW_FAILED => "the adapter refused to tune to that network\n",
                _ => "the join did not complete\n",
            });
            if rc == WIFI_CONNECTED {
                // The new network has its own resolver, and on a captive portal or a split-horizon
                // corporate DNS it has its own answers for the same names. Carrying the previous
                // link's addresses across is how a machine ends up unable to reach anything on a
                // network that works.
                nyx_net::dns::flush();
                // Same reasoning for the kept connection: a socket opened on the previous network
                // is not going to work on this one, and reusing it would cost a failed request and
                // a retry before we noticed.
                nyx_net::http::close_idle();
                // Saved only on success, so the boot agent can never inherit a typo — the same rule
                // the agent itself follows.
                if wifi_save_network(ssid, psk) {
                    self.output_history.push_str("remembered; it will be rejoined at boot\n");
                }
            }
            let s = self.link_summary();
            self.output_history.push_str(&s);
            return;
        }

        self.output_history.push_str(
            "usage: wifi [status] | list | scan | join <ssid> [pass] | leave | on | off | forget\n",
        );
    }

    /// `fetch <url>` — the end-to-end proof that userspace networking reaches the internet.
    ///
    /// Exercises the whole stack in one command: DNS on the active link, a routed TCP socket, the
    /// `std::net` PAL, and — for `https://` — a real TLS 1.3 handshake against the system roots.
    /// Before the socket syscalls learned about the WiFi stack this could not work at all on this
    /// laptop, because the only working link is the radio.
    ///
    /// Everything above the socket lives in `nyx_net`, so this command and the browser share one
    /// HTTP implementation — redirects, chunked bodies and TLS included. A failure here is a failure
    /// the browser would have had too, which is the point of keeping the command.
    fn do_fetch(&mut self, arg: &str) {
        if arg.is_empty() {
            self.output_history.push_str("usage: fetch <url>   (http:// or https://)\n");
            return;
        }

        // Print the link state up front: a DNS failure is almost always "there is no usable link"
        // rather than a real name-resolution problem, and this says which without needing serial.
        self.output_history.push_str(&self.link_summary());
        self.output_history.push_str(&format!("GET {arg}\n"));

        let resp = match nyx_net::get(arg) {
            Ok(r) => r,
            Err(e) => {
                self.output_history.push_str(&format!("  failed: {e}\n"));
                return;
            }
        };

        // Report where the body actually came from — with redirects followed silently, the final URL
        // is the only way to tell that `http://x` quietly became `https://www.x/`.
        self.output_history.push_str(&format!(
            "  {} | {} bytes | {}\n  from {}\n",
            resp.status,
            resp.body.len(),
            resp.content_type().unwrap_or("(no content-type)"),
            resp.url
        ));

        // Only the head of the response goes on screen — a full page would blow out the scrollback,
        // and the byte count above is what actually proves the transfer.
        let text = resp.text();
        // Cut on a char boundary, not a byte index: `text` comes from from_utf8_lossy, so a fixed
        // 1024 can land mid-sequence and slicing there panics.
        let cut = text
            .char_indices()
            .map(|(i, _)| i)
            .chain(core::iter::once(text.len()))
            .take_while(|&i| i <= 1024)
            .last()
            .unwrap_or(0);
        self.output_history.push_str(&format!("--- head ---\n{}\n", &text[..cut]));
    }

    /// `date` — what this machine believes the time is, from both ends of the chain.
    ///
    /// ★ Not a convenience command. HTTPS depends on it: rustls checks the certificate's validity
    /// window against `SystemTime::now()`, so a clock that is wrong by more than a certificate's
    /// lifetime makes **every** `https://` fetch fail with "certificate not valid yet" — which
    /// looks like a TLS or network bug and is neither.
    ///
    /// It prints the RAW RTC fields (syscall 528) beside the `SystemTime` value that std derives
    /// from them, because those answer different questions. If both are wrong the hardware clock is
    /// wrong and the fix is in the firmware setup screen; if they disagree, the bug is in Nyx's
    /// conversion and no amount of resetting the BIOS will help.
    fn do_date(&mut self) {
        let t = sys_get_rtc();
        self.output_history.push_str(&format!(
            "RTC (syscall 528, raw):  {:04}-{:02}-{:02} {:02}:{:02}:{:02}\n",
            t.year, t.month, t.day, t.hour, t.min, t.sec
        ));

        match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => {
                let secs = d.as_secs() as i64;
                self.output_history.push_str(&format!(
                    "SystemTime (std):        {}  (unix {})\n",
                    fmt_unix(secs),
                    secs
                ));
                // Local time is DERIVED here, not stored. The clock itself never leaves UTC.
                let off = tz_offset_min();
                if off != 0 {
                    self.output_history.push_str(&format!(
                        "Local (UTC{}):        {}\n",
                        fmt_offset(off),
                        fmt_unix(secs + off * 60).replace(" UTC", "")
                    ));
                }
                // 2025-01-01. Anything before this is older than the certificates in use today, so
                // it is worth naming as the cause rather than leaving it to be rediscovered.
                if secs < 1_735_689_600 {
                    self.output_history.push_str(
                        "\n  ** This clock is in the past. TLS certificates will be rejected as\n\
                         \x20    'not valid yet' and every https:// fetch will fail. Set the date in\n\
                         \x20    the firmware setup screen, or use `http://` for now.\n",
                    );
                }
            }
            Err(_) => self
                .output_history
                .push_str("SystemTime (std):        before the epoch (clock unreadable)\n"),
        }
    }

    /// `tz` / `tz +5:30` — show or set the DISPLAY timezone.
    ///
    /// ★ Sets how time is PRINTED, never what the clock holds. The RTC and `SystemTime` stay in UTC
    /// permanently, because everything that uses time for correctness rather than for a human wants
    /// UTC: TLS certificate windows, HTTP `Date:` headers, file timestamps. Storing local time in
    /// the hardware clock is the Windows convention, and it is why dual-boot machines end up
    /// fighting over the RTC — and it would walk straight back into the "certificate not valid yet"
    /// failure that this whole clock effort existed to fix.
    fn do_tz(&mut self, arg: &str) {
        let arg = arg.trim();
        if arg.is_empty() {
            let off = tz_offset_min();
            self.output_history
                .push_str(&format!("display timezone: UTC{}\n", fmt_offset(off)));
            if off == 0 {
                self.output_history
                    .push_str("  (not set — times are shown in UTC. India is `tz +5:30`)\n");
            }
            return;
        }
        let Some(min) = parse_offset(arg) else {
            self.output_history
                .push_str("usage: tz +5:30   (offset from UTC; India is +5:30, no DST)\n");
            return;
        };
        if syscall(SYS_SET_TZ, min as u64, 0, 0, 0, 0, 0) == 0 {
            self.output_history
                .push_str(&format!("display timezone set to UTC{} (saved)\n", fmt_offset(min)));
            self.do_date();
        } else {
            self.output_history.push_str("that is not a real UTC offset\n");
        }
    }

    /// `passwd <phrase>` / `passwd off` — set or clear the Meridian lock's passphrase.
    ///
    /// ⚠️ **The passphrase is typed in the clear and stays in the scrollback.** That is honest for
    /// what this is: a `no_std`-derived shell with no `read_password`, no terminal echo control and
    /// no `getpass`. It is called out here rather than hidden because the alternative — a prompt
    /// that *looks* private on a terminal that cannot make it private — would be worse. Run
    /// `clear` afterwards.
    ///
    /// ⚠️ And say plainly what it buys: Nyx has no disk encryption, so `auth.bin` gates a drawing
    /// routine and nothing else. This stops a person borrowing your desk. It does not stop a person
    /// with a screwdriver.
    fn do_passwd(&mut self, arg: &str) {
        const PATH: &str = "/mnt/nvme/auth.bin";
        if arg.is_empty() {
            let set = std::fs::metadata(PATH).is_ok();
            self.output_history.push_str(if set {
                "A lock passphrase is set. The lock appears when the machine wakes from idle.\n"
            } else {
                "No lock passphrase is set, so the machine never locks.\n"
            });
            self.output_history.push_str("usage: passwd <phrase>   |   passwd off\n");
            self.output_history.push_str(
                "NOTE: typed in the clear and left in the scrollback — run `clear` afterwards.\n",
            );
            self.output_history.push_str(
                "NOTE: Nyx has no disk encryption. This gates the screen, not the disk.\n",
            );
            return;
        }
        if arg == "off" {
            match std::fs::remove_file(PATH) {
                Ok(_) => self.output_history.push_str("Passphrase cleared. The machine no longer locks.\n"),
                Err(_) => self.output_history.push_str("No passphrase was set.\n"),
            }
            return;
        }

        // Calibrate the iteration count on THIS CPU rather than copying WPA2's 4096, which is
        // calibrated for a handshake a radio is waiting on and is far too low for a password store.
        // Measure a probe, scale to the target, and write the count into the file so it can be
        // raised later without invalidating the passphrase that is already stored.
        const PROBE: u32 = 20_000;
        const TARGET_MS: usize = 250;
        let t0 = nyx_api::sys_get_time();
        let _ = nyx_crypto::pbkdf2_sha1(arg.as_bytes(), b"calibrate", PROBE, 32);
        let probe_ms = nyx_api::sys_get_time().saturating_sub(t0).max(1);
        // Clamp hard at both ends. A clock that returned nonsense must not write an iteration count
        // that makes every future unlock take a minute, nor one that makes it free.
        let iters = ((PROBE as usize * TARGET_MS / probe_ms) as u32).clamp(20_000, 4_000_000);

        // The salt has to differ per installation or two machines with the same passphrase get the
        // same stored key. The RTC is the only entropy this box will answer with — there is no
        // getrandom and no RDRAND wrapper — so it is stretched through SHA-1 with the machine's own
        // SMBIOS string, which is the same construction `Seed::strike` uses for the Entity.
        let mut hw = [0u8; 256];
        let n = nyx_api::sys_get_hw_info(&mut hw);
        let mut seed_src = Vec::new();
        seed_src.extend_from_slice(&raw_unix_now().to_le_bytes());
        seed_src.extend_from_slice(&(nyx_api::sys_get_time() as u64).to_le_bytes());
        seed_src.extend_from_slice(&hw[..n]);
        let salt = &nyx_crypto::sha1(&seed_src)[..16];

        let key = nyx_crypto::pbkdf2_sha1(arg.as_bytes(), salt, iters, 32);

        let mut buf = Vec::with_capacity(53);
        buf.push(1u8); // version
        buf.extend_from_slice(&iters.to_le_bytes());
        buf.extend_from_slice(salt);
        buf.extend_from_slice(&key);

        match std::fs::write(PATH, &buf) {
            Ok(_) => {
                self.output_history.push_str(&format!(
                    "Passphrase set. {} iterations (~{} ms to derive on this CPU).\n",
                    iters, TARGET_MS
                ));
                self.output_history.push_str(
                    "The lock appears the next time the machine wakes from idle.\n",
                );
                self.output_history.push_str(
                    "Run `clear` — the phrase you just typed is still in the scrollback.\n",
                );
            }
            Err(e) => {
                self.output_history
                    .push_str(&format!("Could not write {}: {}\n", PATH, e));
            }
        }
    }

    /// `date offset <secs>` — apply a clock correction directly, with NO network.
    ///
    /// ★ This exists to split `time sync` in half. That command does two new things at once — a
    /// plain-HTTP transfer and syscall 550 — so when it froze the machine there was no way to say
    /// which. This is the syscall on its own: if it freezes, the fault is the clock offset; if it
    /// returns, the fault is in the network path and syscall 550 is exonerated. One boot, not two.
    fn do_date_offset(&mut self, arg: &str) {
        let Ok(secs) = arg.trim().parse::<i64>() else {
            self.output_history
                .push_str("usage: date offset <seconds>   (e.g. 62208000 for ~2 years forward)\n");
            return;
        };
        let before = raw_unix_now();
        self.output_history
            .push_str(&format!("hardware clock: {}\n", fmt_unix(before)));
        self.output_history
            .push_str(&format!("applying offset {secs} s via syscall 550…\n"));
        syscall(SYS_SET_REALTIME_OFFSET, secs as u64, 0, 0, 0, 0, 0);
        self.output_history.push_str("syscall returned.\n");
        // Read it back through std, which is the path that actually matters for TLS.
        match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => self.output_history.push_str(&format!(
                "corrected time: {}\n", fmt_unix(d.as_secs() as i64))),
            Err(_) => self.output_history.push_str("corrected time: before the epoch\n"),
        }
    }

    /// `time sync [url]` — set the clock from a web server's `Date:` header.
    ///
    /// # Why not NTP
    ///
    /// NTP is what every real OS uses, and it is the right long-term answer. It is not available
    /// here yet: it runs over UDP, and `UdpSocket` in the Nyx std PAL is still `UdpSocket(!)` —
    /// every method returns `unsupported()`. Building it needs UDP `bind` in the kernel and the
    /// whole `send_to`/`recv_from` path in the PAL.
    ///
    /// An HTTP `Date:` header gives the same bootstrap over transport that already works today.
    /// Every HTTP/1.1 response carries one, to the second, and — crucially — it comes back over
    /// **plain http://**, which needs no certificate and therefore no working clock. That breaks
    /// the chicken-and-egg: you cannot fetch the time over https when the wrong time is exactly
    /// what stops https from working.
    ///
    /// # What this is not
    ///
    /// Unauthenticated. Anyone able to intercept the request can set this machine's clock, and
    /// setting a clock backwards is how expired certificates are made to look valid again. Plain
    /// SNTP has the same weakness; the answer to both is NTS or Roughtime, and neither is here.
    /// Fine for making a hobby OS work, not something to rely on.
    fn do_time_sync(&mut self, arg: &str) {
        let arg = arg.trim();
        // Plain http on purpose — see above. A https URL here could not work before the clock does.
        let url = if arg.is_empty() { "http://example.com" } else { arg };
        if url.starts_with("https://") {
            self.output_history.push_str(
                "Use an http:// URL. https needs a valid clock, which is what we are trying to fix.\n",
            );
            return;
        }

        // ★ Breadcrumbs into CMOS around every step. This command hard-freezes the machine and the
        // freeze takes the screen, the log and userspace with it — so the only way to learn WHICH
        // step dies is to write it somewhere that survives the power cycle. The next boot prints
        // the last mark reached. See `postmortem::user_mark`.
        mark(1);
        // The link state up front: "failed to lookup address information" collapses NXDOMAIN, a
        // dead resolver and no link at all into one message, and only one of those is about DNS.
        self.output_history.push_str(&self.link_summary());
        self.output_history.push_str(&format!("Asking {url} for the time…\n"));
        let resp = match nyx_net::get(url) {
            Ok(r) => r,
            Err(e) => {
                mark(0);
                self.output_history.push_str(&format!("  failed: {e}\n"));
                return;
            }
        };
        mark(2);
        let Some(date) = resp.header("date") else {
            mark(0);
            self.output_history
                .push_str("  that server sent no Date header; try another URL\n");
            return;
        };
        let Some(server_unix) = parse_http_date(date) else {
            mark(0);
            self.output_history
                .push_str(&format!("  could not parse Date: {date:?}\n"));
            return;
        };
        mark(3);

        // ★ Sanity-bound the answer before it is allowed anywhere near the clock.
        //
        // This value arrives over PLAIN HTTP and is unauthenticated by construction — it has to be,
        // because you cannot fetch the time over https when a wrong clock is what breaks https.
        // But `SystemTime::now()` is the ONLY input to certificate expiry checking, so anyone able
        // to answer that request could otherwise set this machine's clock at will. Setting it
        // BACKWARDS is the dangerous direction: it makes expired — and, since there is no CRL or
        // OCSP here, revoked — certificates look valid again. And the write is persistent (the RTC
        // is set below), so an attack would survive the power cycle.
        //
        // A floor fixes the whole rollback class for one comparison: this build did not exist
        // before it was compiled, so any "now" earlier than the build stamp is a lie. The ceiling
        // catches the other direction, where a far-future clock would reject every live
        // certificate as not-yet-valid and take the machine off https entirely.
        //
        // This is not authentication and does not pretend to be. NTS or Roughtime is the real
        // answer; this bounds the damage in the meantime.
        if server_unix < BUILD_UNIX {
            self.output_history.push_str(&format!(
                "  REFUSED: {} is before this build was compiled ({}).
                   A clock set backwards makes expired certificates look valid, so this is not
                   accepted from an unauthenticated source.
",
                fmt_unix(server_unix),
                fmt_unix(BUILD_UNIX)
            ));
            mark(0);
            return;
        }
        if server_unix > BUILD_UNIX + MAX_CLOCK_AHEAD_SECS {
            self.output_history.push_str(&format!(
                "  REFUSED: {} is more than {} years after this build.
",
                fmt_unix(server_unix),
                MAX_CLOCK_AHEAD_SECS / (365 * 24 * 3600)
            ));
            mark(0);
            return;
        }

        // The offset is computed against the UNCORRECTED clock, so running `time sync` twice does
        // not apply the correction on top of itself.
        let raw = raw_unix_now();
        mark(4);
        let offset = server_unix - raw;
        self.output_history.push_str(&format!(
            "  server says   {}\n  hardware says {}\n  out by {} s\n",
            fmt_unix(server_unix),
            fmt_unix(raw),
            offset
        ));

        // Write the HARDWARE clock, so this survives the power cycle. The in-RAM offset (550) is a
        // fallback for when that is refused — it fixes the session but is lost on reboot, and a fix
        // you have to reapply after every boot is not a fix.
        if syscall(SYS_SET_RTC, server_unix as u64, 0, 0, 0, 0, 0) == 0 {
            mark(5);
            // ★ Read the HARDWARE back and report what it holds, rather than asserting success.
            //
            // A 0 from the syscall means "the write was issued", not "the clock kept it" — and the
            // line that used to print here claimed the second while only knowing the first. On this
            // machine the RTC comes back at 2024 after every power cycle, so that reassurance was
            // the only thing standing between the user and the truth. Anything that says "this
            // survives a reboot" had better have checked.
            let back = sys_get_rtc();
            self.output_history.push_str(&format!(
                "  RTC reads back {:04}-{:02}-{:02} {:02}:{:02}:{:02}\n",
                back.year, back.month, back.day, back.hour, back.min, back.sec
            ));
            // The seconds will have moved on; a clock that refused the write comes back years away.
            let expected_year = fmt_unix(server_unix)
                .get(..4)
                .and_then(|y| y.parse::<u16>().ok())
                .unwrap_or(0);
            if expected_year != 0 && back.year != expected_year {
                self.output_history.push_str(
                    "  ** the RTC did NOT keep it — the write was accepted and the hardware still\n\
                     \x20    disagrees, so it will be wrong again after a power cycle. Most likely a\n\
                     \x20    dead CMOS battery or firmware reinitialising the clock; set the date in\n\
                     \x20    the firmware setup screen to make it stick.\n",
                );
            } else {
                self.output_history
                    .push_str("  hardware clock updated — verified by reading it back.\n");
            }
        } else {
            // `as u64` keeps the two's-complement bit pattern; the kernel reads it back as i64, so
            // a clock that is AHEAD (negative offset) corrects just as well as one behind.
            syscall(SYS_SET_REALTIME_OFFSET, offset as u64, 0, 0, 0, 0, 0);
            mark(5);
            self.output_history.push_str(
                "  could not write the hardware clock; corrected in software only\n\
                 \x20 (this is lost on reboot).\n",
            );
        }
        match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => self.output_history.push_str(&format!(
                "  clock now reads {}\n", fmt_unix(d.as_secs() as i64))),
            Err(_) => {}
        }
        // Cleared LAST. A breadcrumb left behind by a run that finished would be read next boot as
        // a death at step 6, which is worse than no breadcrumb at all.
        mark(0);
    }

    /// `dns <host>` — resolve a name and say how long it took.
    ///
    /// Separate from `get` on purpose. The kernel resolver collapses NXDOMAIN, timeout and "no
    /// usable link" into one opaque failure, so when a page will not load there is no way to tell a
    /// name problem from a link problem from inside `get`. The ELAPSED TIME is the tell, and it is
    /// the reason this prints milliseconds: a resolver that answers is measured in tens of ms, a
    /// fast failure means nothing was reachable to ask, and a slow one means a real timeout.
    fn do_dns(&mut self, arg: &str) {
        let host = arg.trim();
        if host.is_empty() {
            self.output_history.push_str("usage: dns <host> | dns cache | dns flush\n");
            return;
        }

        // The cache is worth being able to see and clear. An invisible cache is a thing you will
        // eventually blame for something it did not do — and a stale one after a network change is
        // exactly the case where you need to be able to rule it out in one command.
        if host == "cache" {
            let entries = nyx_net::dns::cached_hosts();
            if entries.is_empty() {
                self.output_history.push_str("DNS cache is empty.\n");
            } else {
                self.output_history.push_str("DNS cache:\n");
                for e in entries {
                    self.output_history.push_str(&format!("  {e}\n"));
                }
            }
            return;
        }
        if host == "flush" {
            nyx_net::dns::flush();
            self.output_history.push_str("DNS cache cleared.\n");
            return;
        }

        self.output_history.push_str(&self.link_summary());
        // Show EVERY address the resolver gives, not just the one we would dial. A name with one
        // usable address has no fallback, and that difference is invisible from a single line.
        let all = nyx_net::dns::lookup_all(host, 80).unwrap_or_default();
        if all.len() > 1 {
            self.output_history
                .push_str(&format!("  ({} addresses cached for this name)
", all.len()));
        }
        if nyx_net::dns::lookup(host, 80).is_some() {
            // This command deliberately bypasses the cache — it exists to measure the resolver, and
            // a cache hit measures nothing. Saying so matters because the timing below will not be
            // what `get` pays for the same name.
            self.output_history.push_str(
                "  (this name is cached: `get` would skip the lookup below entirely)\n",
            );
        }

        let started = std::time::Instant::now();
        // Port 80 is arbitrary — `to_socket_addrs` needs one and DNS does not care.
        let result = std::net::ToSocketAddrs::to_socket_addrs(&(host, 80u16));
        let ms = started.elapsed().as_millis();

        match result {
            Ok(addrs) => {
                let list: Vec<String> = addrs.map(|a| a.ip().to_string()).collect();
                if list.is_empty() {
                    self.output_history
                        .push_str(&format!("  {host}: resolved to nothing after {ms} ms\n"));
                } else {
                    self.output_history
                        .push_str(&format!("  {host} -> {} ({ms} ms)\n", list.join(", ")));
                }
            }
            Err(e) => {
                let reading = if ms < 500 {
                    "fast — nothing was reachable to ask (link/stack problem, not the name)"
                } else if ms < 6000 {
                    "the resolver was asked and did not answer in time"
                } else {
                    "the full deadline expired — no server answered at all"
                };
                self.output_history
                    .push_str(&format!("  {host}: FAILED after {ms} ms\n  {reading}\n  ({e})\n"));
            }
        }
    }

    /// `get <url>` — fetch a page and render it as text.
    ///
    /// This is the text browser. The graphical one is unshipped; everything above the socket is the
    /// same `nyx_net` transport, so what this proves, a browser would also have.
    fn do_get(&mut self, arg: &str) {
        let arg = arg.trim();
        if arg.is_empty() {
            self.output_history.push_str("usage: get <url>\n");
            return;
        }
        // Bare host -> https, matching what every mainstream browser now does. Guessing http costs
        // a whole extra request and DNS lookup on any site that redirects, which is most of them.
        let url = if arg.contains("://") { arg.to_string() } else { format!("https://{arg}") };
        self.start_load(&url, Nav::Push);
    }

    /// Begin a page load. Returns immediately; `update` drives it to completion.
    fn start_load(&mut self, url: &str, nav: Nav) {
        if self.loading.is_some() {
            self.output_history
                .push_str("Already loading. `stop` to cancel it first.\n");
            return;
        }
        match nyx_net::Fetch::new(url) {
            Ok(f) => {
                // The link state, up front, exactly as `fetch` and `dns` already do it. A failure
                // to reach anything is far more often "there is no usable link" than a fault in the
                // page fetch, and `get` was the one command that hid the answer.
                let link = self.link_summary();
                self.output_history.push_str(&link);
                // Name the commonest cause outright. Until the kernel's resolver learned to refuse
                // a query with no source address, browsing in this window did not fail — it
                // PANICKED THE MACHINE, inside smoltcp's DNS dispatch. It is now a clean failure,
                // but it is still a failure, and "ip=0.0.0.0" in the line above is easy to skim
                // past when you are looking at a URL.
                if let Some(st) = sys_wifi_status() {
                    if st.ip == [0, 0, 0, 0] {
                        self.output_history.push_str(
                            "  ** no DHCP lease yet — DNS cannot work until this machine has an\n\
                             \x20    address. Wait a few seconds after boot, or `wifi` to check.\n",
                        );
                    }
                }
                self.output_history.push_str(&format!("GET {url}\n"));
                self.status_line = Some("  starting…".to_string());
                self.loading = Some(Loading {
                    fetch: Box::new(f),
                    requested: url.to_string(),
                    nav,
                    started: std::time::Instant::now(),
                    link_at_start: sys_wifi_rx_ring(),
                });
            }
            Err(e) => self.output_history.push_str(&format!("  bad URL: {e}\n")),
        }
    }

    /// One frame's worth of loading. Returns true if anything changed on screen.
    ///
    /// This is the whole reason the window stays alive during a fetch: `Fetch::poll` blocks for at
    /// most a socket timeout and reports a timeout as "nothing yet", so the cost of staying
    /// responsive is bounded by a constant rather than by how slow the server is.
    fn pump_load(&mut self) -> bool {
        let Some(load) = self.loading.as_mut() else { return false };

        let progress = load.fetch.poll();
        let elapsed = load.started.elapsed().as_millis();

        match progress {
            nyx_net::Progress::Connecting(phase) => {
                let host = load.fetch.url().host.clone();
                // After a few seconds a fetch that is merely slow and a fetch that is stuck look
                // identical, and there is no serial console on this machine to tell them apart —
                // so the counters go on screen. Only once it has gone on long enough to be worth
                // suspecting, so a normal load stays quiet.
                let detail = if elapsed > 4000 {
                    format!("\n  {}", load.fetch.diagnostic())
                } else {
                    String::new()
                };
                self.status_line =
                    Some(format!("  {} {host}… ({elapsed} ms){detail}", phase.label()));
                true
            }
            nyx_net::Progress::Receiving { got, total } => {
                // An indeterminate bar is not a failure to measure: a chunked response genuinely
                // has no length, and claiming a percentage would be inventing one.
                self.status_line = Some(match total {
                    Some(t) if t > 0 => format!(
                        "  receiving {} of {} KB ({}%) — {elapsed} ms",
                        got / 1024,
                        t / 1024,
                        got * 100 / t
                    ),
                    _ => format!("  receiving {} KB — {elapsed} ms", got / 1024),
                });
                true
            }
            nyx_net::Progress::Done(resp) => {
                let (nav, requested) = (load.nav, load.requested.clone());
                let before = load.link_at_start;
                // The socket summary on the SUCCESS path too. `rd=N/MB to=T` is what separates
                // "the link is slow" from "we only asked N times": a transfer that spends its life
                // in timeouts is starved, one that reads back-to-back is bandwidth-bound, and the
                // two look identical from a KB/s figure alone.
                let diag = load.fetch.diagnostic();
                self.loading = None;
                self.status_line = None;
                self.finish_load(*resp, nav, &requested, elapsed, before, &diag);
                true
            }
            nyx_net::Progress::Failed(e) => {
                let requested = load.requested.clone();
                // Captured before the fetch is dropped: a failure whose only description is
                // "the server took too long" says nothing about which of the four stages it died
                // in, and that is the entire question.
                let diag = load.fetch.diagnostic();
                self.loading = None;
                self.status_line = None;
                self.output_history.push_str(&format!(
                    "  failed after {elapsed} ms: {e}\n  ({requested})\n  {diag}\n"
                ));
                true
            }
        }
    }

    /// A response arrived: render it, remember it, and put it on screen.
    fn finish_load(
        &mut self,
        resp: nyx_net::Response,
        nav: Nav,
        requested: &str,
        ms: u128,
        before: WifiRing,
        diag: &str,
    ) {
        let final_url = resp.url.to_string();
        let is_html = resp
            .content_type()
            .map(|c| c.to_ascii_lowercase().contains("html"))
            .unwrap_or(true);

        // Charset: the header first, then a `<meta>` inside the document, then UTF-8. The meta only
        // gets a say when the header was silent — a server that declares a charset outranks the
        // document, and a document that disagrees with its server is not ours to arbitrate.
        let charset = resp
            .charset()
            .or_else(|| if is_html { nyx_htmltext::sniff_charset(&resp.body) } else { None });
        let body = resp.text_as(charset.as_deref());

        self.output_history.push_str(&format!(
            "  {} | {} bytes | {} | {} ms\n",
            resp.status,
            resp.body.len(),
            resp.content_type().unwrap_or("(no content-type)"),
            ms
        ));
        // What this transfer cost the link. Deltas, not totals — the question is how THIS fetch
        // went, and frames-per-KB is what separates "the link is slow" from "the link is
        // retransmitting". `tx_dropped` is outbound traffic the driver refused and smoltcp threw
        // away; anything but zero is lost ACKs, which is exactly how a download crawls.
        let now = sys_wifi_rx_ring();
        let rx_kb = now.rx_bytes.saturating_sub(before.rx_bytes) as u128 / 1024;
        let tx_frames = now.tx_frames.saturating_sub(before.tx_frames);
        let dropped = now.tx_nospace.saturating_sub(before.tx_nospace);
        let kbps = if ms > 0 { rx_kb * 1000 / ms } else { 0 };
        self.output_history.push_str(&format!(
            "  link: {} KB in {} ms = {} KB/s   tx {} frames   tx_dropped {}
  {diag}
",
            rx_kb, ms, kbps, tx_frames, dropped
        ));

        // Only worth a line when it differs: with redirects followed silently, the final URL is the
        // only way to tell that `http://x` quietly became `https://www.x/`.
        if final_url != requested {
            self.output_history.push_str(&format!("  from {final_url}\n"));
        }

        let page = LoadedPage {
            url: final_url.clone(),
            title: String::new(),
            text: String::new(),
            links: Vec::new(),
            base: None,
            source: body,
            is_html,
        };
        self.browser.page = Some(page);
        self.render_current();

        if nav == Nav::Push {
            self.browser.visit(&final_url);
        } else if let Some(slot) = self.browser.history.get_mut(self.browser.pos) {
            // back/forward/reload land on the post-redirect URL, which may differ from the one the
            // history recorded. Correcting it in place keeps `back` from bouncing through the
            // redirect again on the way out.
            *slot = final_url;
        }
        // A new page invalidates the previous search.
        self.browser.find_from = 0;
    }

    /// Render the current page's stored source into text, and print it.
    ///
    /// Separate from the load so `reader` can re-render without another request. That matters more
    /// than it sounds: refetching to change a rendering option would put a TLS handshake behind a
    /// display toggle.
    fn render_current(&mut self) {
        let Some(page) = self.browser.page.as_mut() else {
            self.output_history.push_str("No page loaded. `get <url>` first.\n");
            return;
        };

        if page.is_html {
            let opts = nyx_htmltext::Options { reader: self.browser.reader };
            let rendered = nyx_htmltext::render_with(&page.source, &opts);
            page.title = rendered.title;
            page.text = rendered.text;
            page.links = rendered.links;
            page.base = rendered.base;
        } else {
            // Not markup: show it as-is rather than running a tag stripper over JSON or plain text.
            page.title.clear();
            page.text.clone_from(&page.source);
            page.links.clear();
            page.base = None;
        }

        let title = page.title.clone();
        let text = page.text.clone();
        let n_links = page.links.len();
        let reader = self.browser.reader;

        if !title.is_empty() {
            self.output_history.push_str(&format!("\n== {title} ==\n"));
        }
        self.output_history.push('\n');
        self.output_history.push_str(&clip(&text, MAX_PAGE_CHARS));
        self.output_history.push_str(&format!(
            "\n[{} line{}, {n_links} link{}{}] — `links`, `open <n>`, `find <text>`\n",
            text.lines().count(),
            if text.lines().count() == 1 { "" } else { "s" },
            if n_links == 1 { "" } else { "s" },
            if reader { ", reader on" } else { "" },
        ));
    }

    /// `links` — list what the last page linked to.
    fn do_links(&mut self) {
        let Some(page) = self.browser.page.as_ref() else {
            self.output_history.push_str("No page loaded. `get <url>` first.\n");
            return;
        };
        if page.links.is_empty() {
            self.output_history.push_str("This page has no links.\n");
            return;
        }
        let mut s = String::new();
        for l in &page.links {
            let label = if l.text.is_empty() { "(no text)" } else { l.text.as_str() };
            s.push_str(&format!("  [{}] {}\n      {}\n", l.index, clip_line(label, 60), l.href));
        }
        self.output_history.push_str(&s);
    }

    /// `open <n>` — follow one of those links — or `open <url>`, which is `get` by another name.
    fn do_open(&mut self, arg: &str) {
        let arg = arg.trim();
        if arg.is_empty() {
            self.output_history.push_str("usage: open <n>   (see `links`)   |   open <url>\n");
            return;
        }

        // A bare number is a link index; anything else is a URL. Checked in that order because the
        // index is the common case and a hostname is never all digits.
        let Ok(n) = arg.parse::<usize>() else {
            let url = if arg.contains("://") { arg.to_string() } else { format!("https://{arg}") };
            self.start_load(&url, Nav::Push);
            return;
        };

        let Some(page) = self.browser.page.as_ref() else {
            self.output_history.push_str("No page loaded. `get <url>` first.\n");
            return;
        };
        let Some(link) = page.links.iter().find(|l| l.index == n).cloned() else {
            self.output_history.push_str(&format!("No link [{n}]. Try `links`.\n"));
            return;
        };

        // Resolve through `Url::join`, which handles absolute, scheme-relative (`//host/x`) and
        // path-relative forms, and now also `.`/`..` — string concatenation is how `/a/b` + `../c`
        // becomes `/a/b/../c`, a path the server has never heard of.
        let target = match self.browser.base_url() {
            Some(base) => match nyx_net::url::Url::parse(base).and_then(|b| b.join(&link.href)) {
                Ok(u) => u.to_string(),
                Err(e) => {
                    self.output_history.push_str(&format!("Bad link {:?}: {e}\n", link.href));
                    return;
                }
            },
            None => link.href.clone(),
        };
        self.start_load(&target, Nav::Push);
    }

    /// `back` / `forward` — move through this session's history without refetching from `open`.
    fn do_back(&mut self, delta: isize) {
        let b = &self.browser;
        if b.history.is_empty() {
            self.output_history.push_str("Nowhere to go — no pages visited yet.\n");
            return;
        }
        let target = b.pos as isize + delta;
        if target < 0 {
            self.output_history.push_str("Already at the oldest page.\n");
            return;
        }
        if target as usize >= b.history.len() {
            self.output_history.push_str("Already at the newest page.\n");
            return;
        }
        self.browser.pos = target as usize;
        let url = self.browser.history[self.browser.pos].clone();
        self.start_load(&url, Nav::Keep);
    }

    /// `reload` — fetch the current page again.
    fn do_reload(&mut self) {
        let Some(url) = self.browser.history.get(self.browser.pos).cloned() else {
            self.output_history.push_str("Nothing to reload. `get <url>` first.\n");
            return;
        };
        self.start_load(&url, Nav::Keep);
    }

    /// `history` — where this session has been, with the current page marked.
    fn do_history(&mut self) {
        if self.browser.history.is_empty() {
            self.output_history.push_str("No pages visited yet.\n");
            return;
        }
        let mut s = String::new();
        for (i, url) in self.browser.history.iter().enumerate() {
            s.push_str(&format!(
                "{} {:>3}  {}\n",
                if i == self.browser.pos { "*" } else { " " },
                i + 1,
                url
            ));
        }
        s.push_str("  `back` / `forward` to move, `open <n>` is links not history\n");
        self.output_history.push_str(&s);
    }

    /// `reader` — toggle dropping nav/aside/footer, and re-render what is already loaded.
    fn do_reader(&mut self) {
        self.browser.reader = !self.browser.reader;
        self.output_history.push_str(if self.browser.reader {
            "Reader mode ON — navigation, sidebars and footers are dropped.\n"
        } else {
            "Reader mode OFF — the whole document is shown.\n"
        });
        if self.browser.page.is_some() {
            self.render_current();
        }
    }

    /// `find <text>` / `next` — search the page that is loaded, not the scrollback.
    ///
    /// Searching the page rather than the terminal's output is the distinction that makes this
    /// useful: the scrollback also holds the output of every other command, and a hit in last
    /// week's `acpi log` is not what "find" means while reading a page.
    fn do_find(&mut self, arg: &str) {
        let term = arg.trim();
        if !term.is_empty() {
            self.browser.find_term = term.to_lowercase();
            self.browser.find_from = 0;
        }
        if self.browser.find_term.is_empty() {
            self.output_history.push_str("usage: find <text>   then `next` for the one after\n");
            return;
        }

        let Some(page) = self.browser.page.as_ref() else {
            self.output_history.push_str("No page loaded. `get <url>` first.\n");
            return;
        };

        let needle = self.browser.find_term.clone();
        let from = self.browser.find_from;
        let hit = page
            .text
            .lines()
            .enumerate()
            .skip(from)
            .find(|(_, line)| line.to_lowercase().contains(&needle));

        match hit {
            Some((idx, line)) => {
                self.browser.find_from = idx + 1;
                let total = page.text.lines().filter(|l| l.to_lowercase().contains(&needle)).count();
                let remaining = page
                    .text
                    .lines()
                    .skip(idx + 1)
                    .filter(|l| l.to_lowercase().contains(&needle))
                    .count();
                let text = format!(
                    "  line {}: {}\n  ({} of {} — `next` for the following one)\n",
                    idx + 1,
                    line.trim(),
                    total - remaining,
                    total
                );
                self.output_history.push_str(&text);
            }
            None if from > 0 => {
                // Wrapping is what a reader expects; saying so is what stops it looking like a bug.
                self.browser.find_from = 0;
                self.output_history
                    .push_str(&format!("  no more matches for {needle:?} — wrapped to the top\n"));
            }
            None => {
                self.output_history.push_str(&format!("  {needle:?} is not on this page\n"));
            }
        }
    }

    /// Move up or down through the command history, replacing the input line.
    ///
    /// `cmd_pos == cmd_history.len()` is the live line. Stepping up off it stashes whatever was
    /// half-typed, so arrowing all the way back down returns it rather than leaving a blank prompt —
    /// the behaviour of every shell, and its absence is noticed immediately.
    fn walk_history(&mut self, up: bool) {
        if self.cmd_history.is_empty() {
            return;
        }
        let live = self.cmd_history.len();

        if up {
            if self.cmd_pos == 0 {
                return; // already at the oldest
            }
            if self.cmd_pos == live {
                self.cmd_stash.clear();
                self.cmd_stash.push_str(&self.input_buffer);
            }
            self.cmd_pos -= 1;
        } else {
            if self.cmd_pos >= live {
                return; // already on the live line
            }
            self.cmd_pos += 1;
        }

        self.input_buffer.clear();
        if self.cmd_pos == live {
            let stash = core::mem::take(&mut self.cmd_stash);
            self.input_buffer.push_str(&stash);
        } else {
            let entry = self.cmd_history[self.cmd_pos].clone();
            self.input_buffer.push_str(&entry);
        }
    }

    /// Record a command in the history. Consecutive duplicates are collapsed, because pressing Up
    /// four times to get past four `reload`s is not history, it is an obstacle.
    fn remember_command(&mut self, cmd: &str) {
        if cmd.is_empty() {
            return;
        }
        if self.cmd_history.last().map(String::as_str) != Some(cmd) {
            self.cmd_history.push(cmd.to_string());
            // Bounded for the same reason the scrollback is: a session should not grow without end.
            if self.cmd_history.len() > 200 {
                self.cmd_history.remove(0);
            }
        }
        self.cmd_pos = self.cmd_history.len();
        self.cmd_stash.clear();
    }

    /// `stop` — abandon a load in flight.
    fn do_stop(&mut self) {
        match self.loading.take() {
            // Dropping the `Fetch` closes its socket. There is nothing else to unwind: the stepped
            // fetch owns its whole state, which is the reason it can be abandoned at any point.
            Some(l) => {
                self.status_line = None;
                self.output_history
                    .push_str(&format!("Stopped loading {}\n", l.requested));
            }
            None => self.output_history.push_str("Nothing is loading.\n"),
        }
    }

    /// `keepalive [on|off]` — switch HTTP connection reuse at runtime.
    ///
    /// A debug switch, deliberately: every hardware test here costs a power cycle, so deciding
    /// whether a feature helps by building it twice costs two. Keep-alive is currently *suspected of
    /// making things worse* — three back-to-back fetches ran 45 s, 30 s and 25 s against 12.5 s for
    /// an isolated one — and with this the comparison is four commands in one boot:
    ///
    /// ```text
    /// keepalive off ; get <url> ; get <url>
    /// keepalive on  ; get <url> ; get <url>
    /// ```
    fn do_keepalive(&mut self, arg: &str) {
        match arg {
            "" => {
                let state = if nyx_net::http::keep_alive_enabled() { "on" } else { "off" };
                self.output_history
                    .push_str(&format!("keepalive is {state} (usage: keepalive on|off)
"));
            }
            "on" | "off" => {
                let on = arg == "on";
                nyx_net::http::set_keep_alive(on);
                self.output_history.push_str(&format!(
                    "keepalive {arg} — {}
",
                    if on {
                        "connections may be reused between requests"
                    } else {
                        // set_keep_alive(false) also drops whatever is currently held, so the very
                        // next `get` dials fresh rather than inheriting a connection from before.
                        "every request dials a fresh connection; any held one was closed"
                    }
                ));
            }
            other => self
                .output_history
                .push_str(&format!("keepalive: expected on|off, got {other:?}
")),
        }
    }

    // D4: `toolchains` — enumerate the registry's installed language backends. This is the single
    // place the terminal learns what languages it can build; adding one is a one-line change in
    // Registry::with_defaults(), no terminal edit needed.
    fn list_toolchains(&mut self) {
        let reg = Registry::with_defaults();
        self.output_history.push_str("Installed toolchains:\n");
        for b in reg.backends() {
            let exts = b.extensions().join(", .");
            self.output_history.push_str(&format!(
                "  {:<8} .{}  - {}\n",
                b.name(),
                exts,
                b.describe()
            ));
        }
    }

    // D4: `compile <file>` — read a source file via std::fs, hand it to the registry (which picks the
    // backend by extension), and report the result. This is the whole point of Workstream D: the
    // terminal compiles any registered language without knowing which one it is.
    fn do_compile(&mut self, path: &str) {
        let source = match std::fs::read_to_string(path) {
            Ok(s) => s,
            Err(e) => {
                self.output_history.push_str(&format!("compile: cannot read '{path}': {e}\n"));
                return;
            }
        };

        let reg = Registry::with_defaults();
        match reg.compile(path, &source) {
            Ok(Artifact::Text { label, suggested_ext, content }) => {
                let out = replace_ext(path, &suggested_ext);
                match std::fs::write(&out, content.as_bytes()) {
                    Ok(()) => self.output_history.push_str(&format!(
                        "compile: {} -> {} ({} bytes of {})\n",
                        path, out, content.len(), label
                    )),
                    Err(e) => self.output_history.push_str(&format!(
                        "compile: built {} but could not write {out}: {e}\n",
                        label
                    )),
                }
                // Echo the produced text so the result is visible in the terminal itself.
                for line in content.lines() {
                    self.output_history.push_str("  ");
                    self.output_history.push_str(line);
                    self.output_history.push('\n');
                }
            }
            Ok(Artifact::Ran { output }) => {
                self.output_history.push_str("compile: program output:\n");
                for line in output.lines() {
                    self.output_history.push_str("  ");
                    self.output_history.push_str(line);
                    self.output_history.push('\n');
                }
            }
            Ok(Artifact::Executable { bytes }) => {
                self.output_history.push_str(&format!(
                    "compile: produced a {}-byte executable (run support TBD)\n",
                    bytes.len()
                ));
            }
            Err(diags) => {
                self.output_history.push_str(&format!("compile: FAILED ({} diagnostic(s)):\n", diags.len()));
                for d in &diags {
                    self.output_history.push_str(&format!("  {}\n", d.message));
                }
            }
        }
    }
}

// Swap a path's extension for `new_ext` (no dot). If there's no extension, append one. Kept simple:
// operates on the final path component so a dotted directory earlier in the path is left alone.
fn replace_ext(path: &str, new_ext: &str) -> String {
    let slash = path.rfind(|c| c == '/' || c == '\\').map(|i| i + 1).unwrap_or(0);
    match path[slash..].rfind('.') {
        Some(rel_dot) => format!("{}.{}", &path[..slash + rel_dot], new_ext),
        None => format!("{}.{}", path, new_ext),
    }
}

impl NyxApp for TerminalApp {
    fn icon_path(&self) -> &str { "/mnt/nvme/apps/Terminal.nyx/icon.png" }
    // "Nyx Matrix Terminal" until now — the last trace of the green-on-black identity the rest of
    // this file already gave up. The caption line is set in the shell's own type beside every other
    // window's name; a product name there reads as branding on a surface that carries none.
    fn title(&self) -> &str { "Terminal" }
    fn initial_width(&self) -> usize { 640 }
    fn initial_height(&self) -> usize { 400 }

    // ── Scrollback, reported to the shell ───────────────────────────────────────────────────
    //
    // These three are all it takes to get a window-aligned scrollbar: the shell reads `content_h`
    // and `scroll_off` out of the already-mapped window header each frame and sends `MSG_SCROLL`
    // when the thumb is dragged. No new IPC, no new protocol — the terminal was simply the one app
    // that never implemented the interface, which is why its output ran off the bottom for good.
    fn content_height(&self) -> usize {
        self.wrapped_len * TERM_LINE_H + PAD_Y * 2
    }
    fn scroll_offset(&self) -> usize {
        self.scroll * TERM_LINE_H
    }
    fn on_scroll(&mut self, new_offset: usize) -> bool {
        self.set_scroll(new_offset / TERM_LINE_H);
        true
    }

    /// The shell announces its theme on connect and on every toggle (Meridian step 13). Returning
    /// true only on a real change keeps a redundant announcement from costing a repaint.
    fn on_theme(&mut self, dark: bool) -> bool {
        use core::sync::atomic::Ordering;
        DARK.swap(dark, Ordering::Relaxed) != dark
    }

    fn update(&mut self) -> bool {
        // The page loader's frame hook. `update` is called every ~16 ms whether or not anything
        // happened, which is exactly the cadence a stepped fetch wants — and it is the difference
        // between a window that shows its progress and one that is simply frozen until the page
        // arrives.
        let mut redraw = self.pump_load();

        self.blink_timer += 1;
        if self.blink_timer > 30 {
            self.blink_timer = 0;
            self.cursor_visible = !self.cursor_visible;
            redraw = true; // Force redraw to show/hide cursor
        }
        redraw
    }

    fn draw(&mut self, canvas: &mut Canvas) {
        let t = theme();
        canvas.fill_rect(0, 0, canvas.width, canvas.height, term_bg());

        // One advance for the whole face — JetBrains Mono is fixed-pitch, and asking per glyph would
        // take the font mutex once per character of an 8000-character scrollback, every frame.
        let cw = nyx_gui::font::advance_px_face(MONO, 'M', TERM_PX).max(1);

        let inner_w = canvas.width.saturating_sub(PAD_X * 2);
        let inner_h = canvas.height.saturating_sub(PAD_Y * 2);
        let cols = (inner_w / cw).max(1);
        let rows = (inner_h / TERM_LINE_H).max(1);

        self.rewrap(cols);
        self.rebuild_tail(cols);

        // Publish the measurements `content_height`/`scroll_offset` need, then re-clamp: the window
        // may have been resized since the last frame, which changes both the wrap and the viewport.
        self.wrapped_len = self.wrapped.len() + self.tail.len();
        self.view_rows = rows;
        let max = self.max_scroll();
        if self.stick_bottom {
            self.scroll = max;
        } else if self.scroll > max {
            self.scroll = max;
        }

        let total = self.wrapped_len;
        let last_idx = total.saturating_sub(1);

        for (row, idx) in (self.scroll..total).take(rows).enumerate() {
            let text = self.display_line(idx);
            let y = PAD_Y + row * TERM_LINE_H;
            let mut x = PAD_X;

            // `.p` is the prompt sigil, `.o` is the echoed command, everything else is `--fg-2`
            // output. Only a line that actually starts with the sigil gets the treatment, so a
            // wrapped continuation of a long command is not re-coloured halfway through.
            let (sigil, rest) = match text.strip_prefix("N> ") {
                Some(r) => ("N> ", r),
                None => ("", text),
            };

            for ch in sigil.chars() {
                canvas.draw_char_px_face(x, y, ch, t.fg_4, MONO, TERM_PX);
                x += cw;
            }
            let body_color = if sigil.is_empty() { t.fg_2 } else { t.fg };
            for ch in rest.chars() {
                canvas.draw_char_px_face(x, y, ch, body_color, MONO, TERM_PX);
                x += cw;
            }

            // The caret sits at the end of the live input line, and only when that line is on
            // screen — scrolled up into history, there is nothing to blink at.
            if idx == last_idx && self.cursor_visible {
                let cy = y + TERM_LINE_H.saturating_sub(CUR_H) / 2;
                canvas.fill_rect(x, cy, CUR_W, CUR_H, t.accent);
            }
        }
    }

    fn on_key(&mut self, key: char) -> bool {
        self.cursor_visible = true;
        self.blink_timer = 0;

        // Scrollback keys. Handled before anything else so they never reach the input buffer.
        //
        // Keyboard as well as the scrollbar because reading a long dump is the actual use case —
        // `acpi log` is hundreds of lines, and hunting for a thumb with the mouse to page through
        // it is worse than PageUp. A page overlaps by one line so nothing falls between screens.
        {
            let page = self.view_rows.saturating_sub(1).max(1);
            let target = match key {
                keys::PAGE_UP => Some(self.scroll.saturating_sub(page)),
                keys::PAGE_DOWN => Some(self.scroll + page),
                keys::HOME => Some(0),
                keys::END => Some(usize::MAX),
                _ => None,
            };
            if let Some(t) = target {
                self.set_scroll(t);
                return true;
            }
        }

        // Command history. The arrows previously fell through to the input buffer and were typed
        // into the command as literal U+E012/U+E013 glyphs, so Up printed a box. There is no Ctrl on
        // this machine — the kernel builds `pc_keyboard` with `HandleControl::Ignore` — so the
        // arrows are the only key left that can carry this.
        if key == keys::UP || key == keys::DOWN {
            self.walk_history(key == keys::UP);
            return true;
        }

        if key == '\n' || key == '\r' {
            // Typing always returns you to the live end of the output — a command whose result you
            // could not see because the view was parked in history would be baffling.
            self.stick_bottom = true;
            // Own the command string so the dispatch below can take `&mut self` (do_compile /
            // list_toolchains) without colliding with a borrow of self.input_buffer.
            let cmd = self.input_buffer.trim().to_string();
            self.remember_command(&cmd);
            let cmd = cmd.as_str();
            self.output_history.push_str("N> ");
            self.output_history.push_str(cmd);
            self.output_history.push('\n');

            if cmd == "help" {
                self.output_history.push_str("Commands: help, clear, echo <text>, settings, explorer, sysmon\n");
                self.output_history.push_str("  toolchains        - list installed language compilers\n");
                self.output_history.push_str("  compile [file]    - compile a source file (defaults to the bundled sample.ql)\n");
                self.output_history.push_str("Browsing (text mode — this is the browser now):\n");
                self.output_history.push_str("  get <url>         - fetch a page and render it as text (bare host => https)\n");
                self.output_history.push_str("  links             - list this page's links\n");
                self.output_history.push_str("  open <n> | <url>  - follow link <n>, or go straight to a URL\n");
                self.output_history.push_str("  back | b          - the previous page      forward | f - undo a back\n");
                self.output_history.push_str("  reload | r        - fetch this page again  history     - everywhere this session went\n");
                self.output_history.push_str("  find <text>       - search THIS PAGE (not the scrollback)   next | n - the following hit\n");
                self.output_history.push_str("  reader            - drop nav/sidebar/footer chrome, re-render with no refetch\n");
                self.output_history.push_str("  page              - re-print the loaded page   stop - abandon a load in flight\n");
                self.output_history.push_str("  keepalive on|off  - reuse connections between requests (debug switch; off dials fresh)\n");
                self.output_history.push_str("  Up / Down arrows  - command history (there is no Ctrl-R on this machine)\n");
                self.output_history.push_str("  dns <host>        - resolve a name, with timing (says WHY a lookup failed)\n");
                self.output_history.push_str("  dns cache | flush - what is cached / drop it (do this after joining a new network)\n");
                self.output_history.push_str("  date              - the clock. A wrong clock makes EVERY https:// fail\n");
                self.output_history.push_str("  time sync [url]   - set the clock from a plain-http server's Date header\n");
                self.output_history.push_str("  tz [+5:30]        - DISPLAY timezone (clock itself stays UTC). India: tz +5:30\n");
                self.output_history.push_str("  date offset <s>   - apply a clock offset directly, no network (bisects time sync)\n");
                self.output_history.push_str("  fetch <url>       - raw GET: status, size, first 1 KB unrendered. BLOCKS (`get` does not)\n");
                self.output_history.push_str("  exec <path>       - fork+execve any binary (a bad one kills this window, not the desktop)\n");
                self.output_history.push_str("Hardware probes:\n");
                self.output_history.push_str("  panel             - what ACPI says about the backlight: _BCL levels and _BQC current (READ ONLY)\n");
                self.output_history.push_str("  panel set <5-100> - drive the panel via _BCM. On this laptop that ends in a firmware SMI\n");
                self.output_history.push_str("  acpi log          - everything ACPICA printed since boot. Read this FIRST when anything ACPI is missing\n");
                self.output_history.push_str("  battery | bat     - ACPI control-method battery: charge, rate, health (READ ONLY)\n");
                self.output_history.push_str("  acpi ls [path]    - walk the ACPI namespace   acpi probe <n> [depth] - one evaluation\n");
                self.output_history.push_str("  ec | ec dump      - raw EC register dump      ec find <n> - search the EC for a value\n");
                self.output_history.push_str("Scrollback:\n");
                self.output_history.push_str("  PageUp / PageDown - page through history   Home / End - jump to top / live end\n");
                self.output_history.push_str("  (or drag the scrollbar; new output follows only when you are at the bottom)\n");
                self.output_history.push_str("Fatal-screen self-test (proves a crash can still report itself):\n");
                self.output_history.push_str("  fault segv        - child derefs null   -> amber banner, desktop survives\n");
                self.output_history.push_str("  fault ud          - child runs ud2      -> amber banner, desktop survives\n");
                self.output_history.push_str("  fault panic       - KILLS THE MACHINE   -> full red screen, then power cycle\n");
                self.output_history.push_str("  passwd [phrase]   - set/show the lock passphrase ('passwd off' clears it)\n");
                self.output_history.push_str("Network (this is the whole radio; the graphical picker was retired in step 20):\n");
                self.output_history.push_str("  wifi              - link state, lease and which network is remembered\n");
                self.output_history.push_str("  wifi list / scan  - the last sweep / a fresh one (scan BLOCKS ~7 s, refused while joined)\n");
                self.output_history.push_str("  wifi join <ssid> [pass]  - join and remember. BLOCKS ~10 s. Passphrase goes in the clear\n");
                self.output_history.push_str("  wifi leave | on | off | forget\n");
            } else if cmd == "clear" {
                self.output_history.clear();
            } else if cmd == "toolchains" {
                self.list_toolchains();
            } else if cmd == "compile" {
                self.do_compile(DEFAULT_SAMPLE);
            } else if let Some(arg) = cmd.strip_prefix("compile ") {
                self.do_compile(arg.trim());
            } else if let Some(arg) = cmd.strip_prefix("fetch ") {
                self.do_fetch(arg.trim());
            } else if let Some(arg) = cmd.strip_prefix("get ") {
                self.do_get(arg);
            } else if let Some(arg) = cmd.strip_prefix("dns ") {
                self.do_dns(arg);
            } else if cmd == "wifi" || cmd.starts_with("wifi ") {
                self.do_wifi(cmd["wifi".len()..].trim());
            } else if cmd == "passwd" || cmd.starts_with("passwd ") {
                self.do_passwd(cmd["passwd".len()..].trim());
            } else if cmd == "date" {
                self.do_date();
            } else if cmd == "tz" {
                self.do_tz("");
            } else if let Some(arg) = cmd.strip_prefix("tz ") {
                self.do_tz(arg);
            } else if let Some(arg) = cmd.strip_prefix("date offset ") {
                self.do_date_offset(arg);
            } else if cmd == "time sync" {
                self.do_time_sync("");
            } else if let Some(arg) = cmd.strip_prefix("time sync ") {
                self.do_time_sync(arg);
            } else if cmd == "links" {
                self.do_links();
            } else if let Some(arg) = cmd.strip_prefix("open ") {
                self.do_open(arg);
            // ── Browsing ────────────────────────────────────────────────────────────────────────
            } else if cmd == "back" || cmd == "b" {
                self.do_back(-1);
            } else if cmd == "forward" || cmd == "f" {
                self.do_back(1);
            } else if cmd == "reload" || cmd == "r" {
                self.do_reload();
            } else if cmd == "history" {
                self.do_history();
            } else if cmd == "reader" {
                self.do_reader();
            } else if cmd == "stop" {
                self.do_stop();
            } else if cmd == "keepalive" || cmd.starts_with("keepalive ") {
                self.do_keepalive(cmd["keepalive".len()..].trim());
            } else if cmd == "page" {
                // Re-print what is already loaded, for when the page has scrolled out of reach.
                self.render_current();
            } else if let Some(arg) = cmd.strip_prefix("find ") {
                self.do_find(arg);
            } else if cmd == "next" || cmd == "n" {
                self.do_find("");
            } else if let Some(arg) = cmd.strip_prefix("exec ") {
                // Launch an ARBITRARY binary. The start menu lives in the compositor, and the
                // compositor IS the desktop — so a launch that kills its caller costs a power
                // cycle. Launching from here costs a terminal window instead, and one boot can
                // try several binaries with no rebuild in between.
                //
                // No trailing NUL needed: sys_execve passes (ptr, len) and the kernel trims.
                let path = arg.trim();
                self.output_history.push_str("Exec: ");
                self.output_history.push_str(path);
                self.output_history.push('\n');
                if sys_fork() == 0 { sys_execve(path); sys_exit(1); }
            } else if let Some(arg) = cmd.strip_prefix("fault ") {
                // Drive each fatal-reporting path on purpose. Every one of these used to be
                // invisible: a userspace fault printed to a serial port this laptop has no cable
                // for, and a kernel panic painted the firmware framebuffer, which stopped being
                // scanned out the moment the GPU driver armed its own plane. Testing that from an
                // organic crash means waiting for one and then not being sure what you saw.
                //
                // The two user faults happen in a FORKED CHILD, so they cost this terminal window
                // nothing — same reasoning as `exec` above.
                match arg.trim() {
                    "segv" => {
                        self.output_history.push_str("Forking a child to dereference null...\n");
                        if sys_fork() == 0 {
                            unsafe { core::ptr::read_volatile(0usize as *const u8) };
                            sys_exit(1);
                        }
                    }
                    "ud" => {
                        self.output_history.push_str("Forking a child to execute ud2...\n");
                        if sys_fork() == 0 {
                            unsafe { core::arch::asm!("ud2", options(noreturn)) };
                        }
                    }
                    "panic" => {
                        // No fork: the point is to kill the KERNEL and see the red screen.
                        self.output_history.push_str("Panicking the kernel. Power cycle after.\n");
                        syscall(555, 0x4E59_5846, 0, 0, 0, 0, 0);
                        self.output_history.push_str("  ...syscall returned: self-test rejected.\n");
                    }
                    other => {
                        self.output_history.push_str("Unknown fault kind '");
                        self.output_history.push_str(other);
                        self.output_history.push_str("'. Try: segv, ud, panic\n");
                    }
                }
            } else if cmd == "panel" || cmd.starts_with("panel ") {
                // What ACPI says about the backlight, and optionally a set.
                //
                // On this laptop the PCH PWM register reports nothing, because firmware owns the
                // panel: `_BCM` in the DSDT ends in an SMI. Hand-rolling that mailbox was tried and
                // the firmware ignored it. Going through ACPICA is the supported path — it runs the
                // ASL as written, including the OS-identification handshake the SMM handler wants.
                //
                // The header reports the path the kernel *resolved*, not the one it hoped for. The
                // first version printed the hardcoded path as though it were a finding, so when the
                // lookup failed the screen still asserted where the device was — while the actual
                // fault was that the path string had never been a path at all (a `\_` escape in C
                // silently made it relative, which AcpiGetHandle rejects outright).
                let info = sys_panel_probe();
                self.output_history.push_str("Panel:\n");

                // ★ The PWM duty register — the real measurement, and the control path.
                //
                // Printed FIRST because everything below it comes from the ACPI cache, which only
                // fills when you run `acpi probe`. This line needs no probe: it is a plain MMIO
                // read. If it shows a percentage, brightness works and `panel set` will move it.
                match sys_backlight(0) {
                    Some(p) => self.output_history.push_str(&format!(
                        "  PWM: {}%  (live read of BXT_BLC_PWM_DUTY — this is the control path)\n",
                        p
                    )),
                    None => self.output_history.push_str(
                        "  PWM: unavailable — no PWM backlight found at 0xC8250/54/58\n",
                    ),
                }
                if info.n_levels > 0 {
                    self.output_history.push_str("  _BCL:");
                    for b in info.levels[..info.n_levels].iter() {
                        self.output_history.push_str(&format!(" {}", b));
                    }
                    self.output_history.push('\n');
                    if info.n_levels == 64 {
                        // The DSDT declares `Package (0x67)` — 103 levels. The transport caps at 64,
                        // so the list above stops early and its last value is NOT the panel maximum.
                        self.output_history.push_str(
                            "  (truncated at 64 of 103 levels — the last value is not the maximum)\n",
                        );
                    }
                }
                // ⚠️ `_BQC` on this machine is a cache, not a measurement: the ASL returns the Name
                // `BRT0`, which is initialised to 100 and only ever written by `_BCM`. Expect 100
                // until something sets the brightness this boot, regardless of where the panel is.
                if let Some(p) = info.current {
                    self.output_history
                        .push_str(&format!("  _BQC: {}%  (cached value, not a panel read)\n", p));
                }

                // The kernel's own report. It is the only thing that can see the namespace, so it
                // formats the findings and userspace prints them verbatim — no second wording of
                // the same fact to drift out of sync.
                let mut dbuf = [0u8; 2048];
                let n = sys_panel_diag(&mut dbuf);
                self.output_history
                    .push_str(core::str::from_utf8(&dbuf[..n]).unwrap_or("  <diag: non-utf8>\n"));
                self.output_history
                    .push_str("  (run `acpi log` for ACPICA's own account of the table load)\n");

                // `panel set <n>` is the only thing here that touches hardware, and it is a separate
                // word on purpose: on this machine it ends in an SMI, and an SMI should never be
                // something you trigger by typing a bare noun.
                let arg = if cmd.len() > 6 { cmd[6..].trim() } else { "" };
                if let Some(v) = arg.strip_prefix("set ") {
                    match v.trim().parse::<u32>() {
                        Ok(want) => {
                            self.output_history.push_str(&match sys_backlight_set(want) {
                                Some(got) => format!(
                                    "  _BCM {} queued -> governor applies {}% within ~1s.\n\
                                     \x20 This is the one call that reaches hardware: it ends in a \
                                     firmware SMI.\n\
                                     \x20 If the screen goes dark or the machine dies, the NEXT \
                                     boot's [USERMARK]\n\
                                     \x20 will read 6 (entered _BCM, never returned) or 106 \
                                     (completed).\n",
                                    want, got
                                ),
                                None => format!("  _BCM {} -> refused\n", want),
                            });
                        }
                        Err(_) => self.output_history.push_str("  usage: panel set <5-100>\n"),
                    }
                } else if !arg.is_empty() {
                    self.output_history.push_str("  usage: panel [set <5-100>]\n");
                }
            } else if cmd == "acpi" || cmd.starts_with("acpi ") {
                // ACPICA's own log. Everything it printed since boot, verbatim.
                //
                // This is the channel that was stubbed out empty for the life of the project, which
                // is why an empty namespace looked like a missing device for three power cycles.
                // Printed raw and unfiltered — the whole point is to see what ACPICA actually said,
                // not a summary of it written by someone who already had a theory.
                let arg = if cmd.len() > 4 { cmd[4..].trim() } else { "" };
                if arg.is_empty() || arg == "log" {
                    // Heap, not stack: the log is 16 KB and this is a std binary.
                    let mut buf = vec![0u8; 16 * 1024];
                    let n = sys_acpi_log(&mut buf);
                    if n == 0 {
                        self.output_history.push_str("ACPICA log is empty.\n");
                    } else {
                        self.output_history.push_str("ACPICA log:\n");
                        self.output_history
                            .push_str(core::str::from_utf8(&buf[..n]).unwrap_or("<non-utf8>\n"));
                        if !self.output_history.at_line_start() { self.output_history.push('\n'); }
                    }
                } else if let Some(n) = arg.strip_prefix("probe ") {
                    // One ACPI evaluation, on the governor's next tick. See sys_acpi_probe.
                    // `acpi probe 1 3` = step 1 at walk depth 3. Depth only applies to step 1.
                    let mut it = n.trim().split_whitespace();
                    let step_txt = it.next().unwrap_or("");
                    let depth: u32 = it.next().and_then(|d| d.parse().ok()).unwrap_or(0);
                    match step_txt.parse::<u8>() {
                        // 7 is the EC dump — added after this range check was written, so
                        // `acpi probe 7` was silently rejected and the dump never ran.
                        // 8 is the stepper walk, 9 attaches the EC address-space handler, 10 runs
                        // `_REG` (which is fatal on this machine — mark 57) and 11 sets `ECRD`
                        // directly, which is what `_REG` exists to do. Extend this range when a step
                        // is added; 7 was silently rejected for a while and its dump never ran.
                        Ok(s) if (1..=12).contains(&s) => {
                            sys_acpi_probe(s, depth);
                            if s == 1 {
                                self.output_history.push_str(&format!(
                                    "Walk depth {}. Breadcrumbs: {} = entered at this depth, \
                                     {} = returned.\n",
                                    if depth == 0 { "unlimited".to_string() } else { depth.to_string() },
                                    60 + if depth < 16 { depth } else { 0 },
                                    80 + if depth < 16 { depth } else { 0 },
                                ));
                            }
                            if s == 1 || s == 8 {
                                // ★ The point of this pass: a walk that dies no longer needs a
                                // power cycle to be located. The cursor is in kernel memory and the
                                // panic screen prints it.
                                self.output_history.push_str(
                                    "If it dies, the RED SCREEN now names the node: walker, node \
                                     index, depth, the 4-char ACPI name, and both pointers.\n\
                                     \x20 Look that name up in nyx-recv/dsdt.dsl.\n\
                                     \x20 Run `acpi probe 8` after `acpi probe 1` — same walk, \
                                     stepper-driven. Same node = bad data, not a bad walker.\n",
                                );
                            }
                            self.output_history.push_str(&format!(
                                "Queued ACPI step {} for the next governor tick (~1s).\n\
                                 \x20 If the machine dies, the NEXT boot's [USERMARK] names it:\n\
                                 \x20   {} = entered step {}, never returned\n\
                                 \x20   {} = step {} completed\n\
                                 {}Then run `panel` / `battery` to see what it captured.\n",
                                s, s, s, 100 + s as u32, s,
                                if s == 5 {
                                    " Step 5 also leaves finer marks:\n\
                                     \x20   50 = entering EC install   51 = EC up, ports known\n\
                                     \x20   52 = entering _STA/_BIF/_BST (first real EC traffic)\n"
                                } else { "" }
                            ));
                        }
                        _ => self.output_history.push_str(
                            "  usage: acpi probe <1-7>\n\
                             \x20   1 namespace walk   2 panel diag   3 _BCL\n\
                             \x20   4 _BQC             5 battery      6 _BCM (apply brightness)\n\
                             \x20   7 EC raw dump      (then `ec` / `ec find <n>`)\n\
                             \x20 NOTE 1 and 5 are known to crash: walks #GP this kernel.\n",
                        ),
                    }
                } else if arg == "ls" || arg.starts_with("ls ") {
                    // The namespace browser — one node per syscall.
                    //
                    // Each name is appended to the scrollback BEFORE the next step is requested, so
                    // if a node takes the machine down its predecessor is already on screen. That is
                    // the whole reason this steps instead of walking: `AcpiWalkNamespace` crashes
                    // this kernel and can only report "it died", never where.
                    let path = arg.strip_prefix("ls").unwrap_or("").trim();
                    let parent = if path.is_empty() { 0 } else { sys_acpi_ns_handle(path) };
                    if !path.is_empty() && parent == 0 {
                        self.output_history
                            .push_str(&format!("  no such ACPI path: {}\n", path));
                    } else {
                        self.output_history.push_str(&format!(
                            "{}:\n",
                            if path.is_empty() { "\\ (root)" } else { path }
                        ));
                        let mut prev = 0u64;
                        let mut n = 0usize;
                        // Handles seen so far, so a chain that never terminates can be characterised
                        // rather than just truncated. The root should have ~9 children; anything
                        // near the cap means the sibling chain is not ending, and WHY matters:
                        //   same handle repeating  -> AcpiGetNextObject is not advancing
                        //   handles cycling        -> the Peer chain is circular (real corruption)
                        //   all distinct           -> RUNAWAY — measured 2026-09-09, see below
                        let mut seen: Vec<u64> = Vec::new();
                        let mut verdict = String::new();
                        // ★★ The parent of the FIRST node. Every genuine sibling shares it.
                        //
                        // This is the check the 512-distinct-handles result demanded. "All distinct,
                        // no repeat" ruled out a cycle but not a runaway, and the two need different
                        // fixes. A node whose `Parent` is not this one is not in this child list at
                        // all — so the index where that first happens is the exact length of the real
                        // chain, and everything after it is memory the walk has no business in.
                        //
                        // ⚠️ Compared against node 0's parent rather than `parent`, because for the
                        // root case userspace holds the ACPI_ROOT_OBJECT sentinel, not the address of
                        // the actual root node. Siblings agreeing with each other is the same test
                        // and needs no such knowledge.
                        let mut first_parent: Option<u64> = None;
                        // ⚠️ Was 512, and that number did real damage: it was quoted for weeks as
                        // "the root has 512 children", which is a CAP being mistaken for a COUNT and
                        // is what made a corrupt namespace look like a broken walker. 4096 is high
                        // enough that hitting it means something is very wrong, and the verdict below
                        // now says so in words instead of leaving a number to be misread.
                        while n < 4096 {
                            match sys_acpi_ns_step(parent, prev) {
                                Some(node) => {
                                    if let Some(i) = seen.iter().position(|&h| h == node.handle) {
                                        verdict = format!(
                                            "  ★ CHAIN REPEATS: handle {:#x} already seen at index \
                                             {} (after {} nodes)\n\
                                             \x20   {}\n",
                                            node.handle, i, n,
                                            if i + 1 == n {
                                                "same handle twice in a row — AcpiGetNextObject is \
                                                 not advancing"
                                            } else {
                                                "the sibling chain is CIRCULAR — this is namespace \
                                                 corruption, and it is why walks never terminate"
                                            }
                                        );
                                        break;
                                    }
                                    // ★ The moment the chain leaves the real child list.
                                    let p0 = *first_parent.get_or_insert(node.parent);
                                    if verdict.is_empty()
                                        && (node.parent != p0 || !node.name_looks_real())
                                    {
                                        let c = node.name4_chars();
                                        verdict = format!(
                                            "  ★ CHAIN RAN AWAY at index {}\n\
                                             \x20   raw name {}{}{}{} ({:#010x}) — {}\n\
                                             \x20   parent   {:#x}, but every node before it had \
                                             {:#x}\n\
                                             \x20   So the real child list is {} long and the walk \
                                             is now in unowned memory.\n",
                                            n, c[0], c[1], c[2], c[3], node.name4,
                                            if node.name_looks_real() {
                                                "a real-looking name, but the wrong parent"
                                            } else {
                                                "not a valid ACPI name — this is not a node"
                                            },
                                            node.parent, p0, n,
                                        );
                                    }
                                    // Handle printed too: names can collide, handles cannot. The raw
                                    // name sits beside the pathname because on a runaway node
                                    // `AcpiGetName` builds a plausible string out of garbage, and
                                    // seeing the two disagree is the tell.
                                    let c = node.name4_chars();
                                    self.output_history.push_str(&format!(
                                        "  {:<10} {:#012x}  p={:#012x}  {}{}{}{}  {}\n",
                                        node.type_name(),
                                        node.handle,
                                        node.parent,
                                        c[0], c[1], c[2], c[3],
                                        node.name_str()
                                    ));
                                    seen.push(node.handle);
                                    prev = node.handle;
                                    n += 1;
                                }
                                None => break,
                            }
                        }
                        if !verdict.is_empty() {
                            self.output_history.push_str(&verdict);
                        } else if n >= 512 {
                            self.output_history.push_str(
                                "  ★ hit the 512 cap with NO repeated handle — the chain is long \
                                 but not looping.\n",
                            );
                        }
                        self.output_history.push_str(&format!("  {} node(s)\n", n));
                    }
                } else {
                    self.output_history.push_str(
                        "  usage: acpi [log | ls [path] | probe <1-6> [depth]]\n\
                         \x20   acpi ls              list the root's children\n\
                         \x20   acpi ls \\_SB.PCI0    list a scope (one node per syscall — the last\n\
                         \x20                        line printed is the last node that was safe)\n",
                    );
                }
            } else if cmd.starts_with("ec find ") {
                // Search EC space for a known 16-bit value.
                //
                // Fedora's ec_sys debugfs interface is gone from modern kernels, so the EC cannot be
                // dumped there and diffed against ours. It does not need to be: some BAT0 fields are
                // STATIC across boots, so they can be searched for here directly.
                //   charge_full  2131 mAh -> 0x0853
                //   design       4474 mAh -> 0x117A
                // Those two are the anchors. Once located, the live fields (charge_now, voltage,
                // current) are almost always in the same struct a few bytes away.
                let want: u32 = cmd[8..].trim().parse().unwrap_or(0);
                let mut ec = [0u8; 256];
                let (state, _, _, n, _) = sys_ec_dump(&mut ec);
                if state != 1 || n == 0 {
                    self.output_history
                        .push_str("EC: not captured — run `ec` on its own, it says why.\n");
                } else if want == 0 || want > 0xFFFF {
                    self.output_history.push_str("  usage: ec find <0-65535>\n");
                } else {
                    let lo = (want & 0xFF) as u8;
                    let hi = ((want >> 8) & 0xFF) as u8;
                    self.output_history
                        .push_str(&format!("Searching EC for {} ({:#06x}):\n", want, want));
                    let mut hits = 0;
                    for i in 0..n.saturating_sub(1) {
                        if ec[i] == lo && ec[i + 1] == hi {
                            self.output_history
                                .push_str(&format!("  offset {:#04x}  little-endian\n", i));
                            hits += 1;
                        } else if ec[i] == hi && ec[i + 1] == lo {
                            self.output_history
                                .push_str(&format!("  offset {:#04x}  BIG-endian\n", i));
                            hits += 1;
                        }
                    }
                    if hits == 0 {
                        self.output_history.push_str(
                            "  no match — the value may be scaled (mWh vs mAh), 8-bit, or not in\n\
                             \x20 EC space at all on this machine.\n",
                        );
                    }
                }
            } else if cmd == "ec" || cmd == "ec dump" {
                // Raw EC register space — the route around the namespace-walk crash.
                //
                // ACPI's _BIF/_BST would give a vendor-neutral battery layout, but reaching them
                // needs an EmbeddedControl handler and installing one walks the namespace, which
                // #GPs this kernel. The EC ports themselves are reachable, so this reads them
                // directly. The map is model-specific: the offsets must be found by correlation.
                let mut ec = [0u8; 256];
                let (state, dport, cport, n, ecstat) = sys_ec_dump(&mut ec);
                if state != 1 {
                    self.output_history.push_str(match state {
                        0 => "EC: nothing probed this boot. Run `acpi probe 7`, wait ~1s, retry.\n",
                        2 => "EC: did not come up — no handle at \\_SB.PCI0.LPCB.ECDV, or _CRS \
                              returned no usable IO ports.\n",
                        3 => "EC: ports found but every read TIMED OUT. The EC is not answering:\n\
                              \x20 likely the data/command ports are swapped, or this EC needs the \
                              burst protocol.\n",
                        _ => "EC: unknown state.\n",
                    });
                    if state == 3 {
                        self.output_history.push_str(&format!(
                            "  ports from _CRS: data {:#06x}, cmd {:#06x}   (Fedora: 0x930 / 0x934)\n\
                             \x20 last EC status byte: {:#04x}\n\
                             \x20   0xff = nothing decoding that port\n\
                             \x20   0x00 = EC idle, but it never raised OBF\n\
                             \x20   else = bit0 OBF, bit1 IBF; a stuck IBF means it never took the \
                             command\n",
                            dport, cport, ecstat & 0xFF
                        ));
                    }
                } else {
                    self.output_history.push_str(&format!(
                        "EC registers (data port {:#06x}, cmd port {:#06x}, {} bytes):\n",
                        dport, cport, n
                    ));
                    for row in 0..(n + 15) / 16 {
                        let base = row * 16;
                        let mut line = format!("  {:02x}: ", base);
                        for i in 0..16 {
                            if base + i < n {
                                line.push_str(&format!("{:02x} ", ec[base + i]));
                            }
                        }
                        line.push('\n');
                        self.output_history.push_str(&line);
                    }
                    // What to look for, so the correlation does not need re-deriving each time.
                    // Fedora reports this machine's BAT0 as ~1486 mAh charge at ~11051 mV.
                    // The anchors are the STATIC fields — they read the same on Fedora and here, so
                    // no simultaneous capture is needed. Fedora's ec_sys debugfs is gone from modern
                    // kernels, which is why we search our own dump rather than diffing against one.
                    self.output_history.push_str(
                        "  Find the battery block with the STATIC anchors (same value every boot):\n\
                         \x20   ec find 2131    last-full charge, mAh  (0x0853)\n\
                         \x20   ec find 4474    design charge, mAh     (0x117A)\n\
                         \x20 Then the live fields sit a few bytes away in the same struct. Dump\n\
                         \x20 again after a while — the bytes that MOVE are charge/current/voltage.\n",
                    );
                }
            } else if cmd == "battery" || cmd == "bat" {
                // ACPI control-method battery. Depends on the namespace actually having loaded, so
                // if this says "no battery" the first thing to check is `acpi log`, not the battery.
                let b = sys_battery();
                if !b.sampled {
                    self.output_history.push_str(
                        "Battery: not sampled yet.\n\
                         \x20 ACPI methods no longer run on their own — a full namespace walk was \
                         crashing the kernel.\n\
                         \x20 Run `acpi probe 5`, wait a second, then `battery` again.\n",
                    );
                } else if !b.ec_ok {
                    self.output_history.push_str(
                        "Battery: the Embedded Controller did not come up.\n\
                         \x20 No battery method was evaluated — doing that without an \
                         EmbeddedControl handler\n\
                         \x20 is what crashed the kernel before, so it is gated.\n\
                         \x20 Likely: PNP0C09 not found, or _CRS gave no usable IO ports.\n",
                    );
                } else if !b.present {
                    self.output_history.push_str(
                        "Battery: EC is up, but no PNP0C0A device reported present.\n\
                         \x20 Either _STA's battery-present bit (0x10) is clear, or _BIF/_BST would \
                         not evaluate.\n\
                         \x20 Fedora on this laptop reports BAT0 present, so expect the former only \
                         if it is unplugged.\n",
                    );
                } else {
                    let (cu, ru) = b.units();
                    let status = if b.critical() { "critical" }
                        else if b.charging() { "charging" }
                        else if b.discharging() { "discharging" }
                        else { "idle" };
                    match b.percent() {
                        Some(p) => self
                            .output_history
                            .push_str(&format!("Battery: {}%  ({})\n", p, status)),
                        None => self
                            .output_history
                            .push_str(&format!("Battery: present, {} (no _BST)\n", status)),
                    }
                    if b.have_bst {
                        self.output_history.push_str(&format!(
                            "  remaining {} {}   rate {} {}   {} mV\n",
                            b.remaining_cap, cu, b.present_rate, ru, b.voltage
                        ));
                    }
                    if b.have_bif {
                        self.output_history.push_str(&format!(
                            "  last full {} {}   design {} {} @ {} mV\n",
                            b.last_full_cap, cu, b.design_cap, cu, b.design_voltage
                        ));
                        if let Some(h) = b.health_percent() {
                            self.output_history
                                .push_str(&format!("  health {}% of design\n", h));
                        }
                    }
                    if let Some(m) = b.minutes_remaining() {
                        self.output_history
                            .push_str(&format!("  about {}h {:02}m left\n", m / 60, m % 60));
                    }
                }
            } else if cmd == "settings" {
                self.output_history.push_str("Launching Settings...\n");
                if sys_fork() == 0 { sys_execve("/mnt/nvme/apps/Settings.nyx/run.bin\0"); sys_exit(1); }
            } else if cmd == "explorer" {
                self.output_history.push_str("Launching Explorer...\n");
                if sys_fork() == 0 { sys_execve("/mnt/nvme/apps/Explorer.nyx/run.bin\0"); sys_exit(1); }
            } else if cmd == "sysmon" {
                self.output_history.push_str("Launching System Monitor...\n");
                if sys_fork() == 0 { sys_execve("/mnt/nvme/apps/SystemMonitor.nyx/run.bin\0"); sys_exit(1); }
            } else if cmd.starts_with("echo ") {
                self.output_history.push_str(&cmd[5..]);
                self.output_history.push('\n');
            } else if !cmd.is_empty() {
                self.output_history.push_str("Unknown command. Type 'help'.\n");
            }
            self.input_buffer.clear();
        } else if key == '\x08' {
            self.input_buffer.pop();
        } else if ('\u{E000}'..='\u{E0FF}').contains(&key) {
            // A key the kernel encodes as a private-use codepoint that this app does not handle —
            // Left, Right, Delete, an unbound F-key. Swallow it. Falling through to the buffer is
            // what used to happen, and it typed a literal box glyph into the middle of the command:
            // silently ignoring a key you have not bound is bad, but printing a box for it is worse,
            // because it looks like a rendering failure rather than an unhandled key.
            return false;
        } else {
            self.input_buffer.push(key);
        }
        true // Redraw instantly on keypress
    }
}

fn main() {
    // Headless breadcrumb in the serial log so the boot test is legible even before the window paints.
    println!("nyx-terminal: D3 std port — shell over nyx-gui on target_os=nyx");

    // Meridian step 11: install JetBrains Mono before the first frame. Only the mono face — see
    // `register_mono`. If it fails to parse, `with_glyph_face` falls back to DejaVu, so the terminal
    // still draws every character; it just loses the fixed pitch (and the wrap, which assumes it).
    if !nyx_meridian::font::register_mono() {
        println!("nyx-terminal: JetBrains Mono failed to parse — falling back to DejaVu");
    }

    // run() takes over the process (event loop, never returns).
    nyx_gui::app::run(TerminalApp::new());
}
