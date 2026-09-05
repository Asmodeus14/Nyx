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

/// `#0B0C0E` — the design gives the terminal its own ground, one step below `--surface`, so a
/// console reads as a recessed well rather than another panel. Taken literally from the CSS.
const TERM_BG_DARK: u32 = 0xFF0B_0C0E;

fn theme() -> nyx_meridian::tokens::Theme {
    nyx_meridian::tokens::Theme::dark()
}

/// JetBrains Mono's slot in `nyx_gui::font`'s registry.
const MONO: usize = nyx_meridian::font::SLOT_MONO;

/// How much of a page goes into the scrollback. The history is one `String` that the draw path
/// walks every frame, so an unbounded paste of a 500 KB page makes the terminal itself unusable.
const MAX_PAGE_CHARS: usize = 8000;

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

struct TerminalApp {
    input_buffer: String,
    output_history: String,
    blink_timer: usize,
    cursor_visible: bool,

    /// The URL the last `get` actually landed on, AFTER redirects. `open <n>` resolves against this
    /// rather than against what was typed: a relative href on a page reached through a 301 is
    /// relative to where the page came FROM, not to where we asked.
    current_url: Option<String>,
    /// Links from the last page, in document order, so `open <n>` has something to mean.
    links: Vec<nyx_htmltext::Link>,

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
}

impl TerminalApp {
    fn new() -> Self {
        Self {
            input_buffer: String::new(),
            output_history: String::from("NyxOS v0.1 Shell\nType 'help' for commands.\n"),
            blink_timer: 0,
            cursor_visible: true,
            current_url: None,
            links: Vec::new(),
            scroll: 0,
            stick_bottom: true,
            wrapped_len: 0,
            view_rows: 1,
        }
    }

    /// Split the scrollback into display lines at `cols` characters.
    ///
    /// JetBrains Mono is fixed-pitch, so wrapping is arithmetic on character counts rather than a
    /// per-glyph advance lookup. That matters: the history is capped at `MAX_PAGE_CHARS` and this
    /// runs every frame, and `advance_px_face` takes a mutex per call.
    ///
    /// Chunks by `chars()`, never by byte index — page text is full of curly quotes and em dashes,
    /// so a byte cut is a routine panic here rather than an edge case (same reason `clip` exists).
    fn wrap_into(&self, cols: usize, out: &mut Vec<String>) {
        out.clear();
        let cols = cols.max(1);
        let prompt_line = format!("N> {}", self.input_buffer);

        for logical in self.output_history.split('\n').chain(core::iter::once(prompt_line.as_str())) {
            if logical.is_empty() {
                out.push(String::new());
                continue;
            }
            let mut cur = String::new();
            let mut n = 0usize;
            for ch in logical.chars() {
                cur.push(ch);
                n += 1;
                if n == cols {
                    out.push(core::mem::take(&mut cur));
                    n = 0;
                }
            }
            if n > 0 {
                out.push(cur);
            }
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
                format!(
                    "wifi: {} ssid={:?} ip={}.{}.{}.{} gw={}.{}.{}.{} dns={}.{}.{}.{}\n",
                    state, st.name(),
                    st.ip[0], st.ip[1], st.ip[2], st.ip[3],
                    st.router[0], st.router[1], st.router[2], st.router[3],
                    st.dns[0], st.dns[1], st.dns[2], st.dns[3],
                )
            }
        }
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
            self.output_history
                .push_str("  hardware clock updated — this survives a reboot.\n");
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
            self.output_history.push_str("usage: dns <host>\n");
            return;
        }
        self.output_history.push_str(&self.link_summary());

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
        self.load(&url);
    }

    /// Fetch `url`, render it, and remember its links for `open`.
    fn load(&mut self, url: &str) {
        self.output_history.push_str(&format!("GET {url}\n"));
        let resp = match nyx_net::get(url) {
            Ok(r) => r,
            Err(e) => {
                self.output_history.push_str(&format!("  failed: {e}\n"));
                return;
            }
        };

        let final_url = resp.url.to_string();
        self.output_history.push_str(&format!(
            "  {} | {} bytes | {}\n  from {}\n",
            resp.status,
            resp.body.len(),
            resp.content_type().unwrap_or("(no content-type)"),
            final_url
        ));

        let is_html = resp
            .content_type()
            .map(|c| c.to_ascii_lowercase().contains("html"))
            .unwrap_or(true);
        let body = resp.text();

        if is_html {
            let page = nyx_htmltext::render(&body);
            if !page.title.is_empty() {
                self.output_history.push_str(&format!("\n== {} ==\n", page.title));
            }
            self.output_history.push('\n');
            self.output_history.push_str(&clip(&page.text, MAX_PAGE_CHARS));
            let n = page.links.len();
            self.links = page.links;
            self.output_history.push_str(&format!(
                "\n[{n} link{}] — `links` to list, `open <n>` to follow\n",
                if n == 1 { "" } else { "s" }
            ));
        } else {
            // Not markup: show it as-is rather than running a tag stripper over JSON or plain text.
            self.links.clear();
            self.output_history.push('\n');
            self.output_history.push_str(&clip(&body, MAX_PAGE_CHARS));
            self.output_history.push('\n');
        }

        // Set LAST, and to the post-redirect URL — `open` resolves relative hrefs against it.
        self.current_url = Some(final_url);
    }

    /// `links` — list what the last page linked to.
    fn do_links(&mut self) {
        if self.links.is_empty() {
            self.output_history.push_str("No links (load a page with `get <url>` first).\n");
            return;
        }
        let mut s = String::new();
        for l in &self.links {
            let label = if l.text.is_empty() { "(no text)" } else { l.text.as_str() };
            s.push_str(&format!("  [{}] {}\n      {}\n", l.index, clip_line(label, 60), l.href));
        }
        self.output_history.push_str(&s);
    }

    /// `open <n>` — follow one of those links.
    fn do_open(&mut self, arg: &str) {
        let Ok(n) = arg.trim().parse::<usize>() else {
            self.output_history.push_str("usage: open <n>   (see `links`)\n");
            return;
        };
        let Some(link) = self.links.iter().find(|l| l.index == n).cloned() else {
            self.output_history.push_str(&format!("No link [{n}]. Try `links`.\n"));
            return;
        };

        // Resolve through `Url::join`, which handles absolute, scheme-relative (`//host/x`) and
        // path-relative forms. Doing it by string concatenation is how `/a/b` + `../c` goes wrong.
        let target = match &self.current_url {
            Some(base) => match nyx_net::url::Url::parse(base) {
                Ok(b) => match b.join(&link.href) {
                    Ok(u) => u.to_string(),
                    Err(e) => {
                        self.output_history.push_str(&format!("Bad link {:?}: {e}\n", link.href));
                        return;
                    }
                },
                Err(e) => {
                    self.output_history.push_str(&format!("Bad base URL: {e}\n"));
                    return;
                }
            },
            None => link.href.clone(),
        };
        self.load(&target);
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
    fn title(&self) -> &str { "Nyx Matrix Terminal" }
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

    fn update(&mut self) -> bool {
        self.blink_timer += 1;
        if self.blink_timer > 30 {
            self.blink_timer = 0;
            self.cursor_visible = !self.cursor_visible;
            return true; // Force redraw to show/hide cursor
        }
        false
    }

    fn draw(&mut self, canvas: &mut Canvas) {
        let t = theme();
        canvas.fill_rect(0, 0, canvas.width, canvas.height, TERM_BG_DARK);

        // One advance for the whole face — JetBrains Mono is fixed-pitch, and asking per glyph would
        // take the font mutex once per character of an 8000-character scrollback, every frame.
        let cw = nyx_gui::font::advance_px_face(MONO, 'M', TERM_PX).max(1);

        let inner_w = canvas.width.saturating_sub(PAD_X * 2);
        let inner_h = canvas.height.saturating_sub(PAD_Y * 2);
        let cols = (inner_w / cw).max(1);
        let rows = (inner_h / TERM_LINE_H).max(1);

        let mut lines: Vec<String> = Vec::new();
        self.wrap_into(cols, &mut lines);

        // Publish the measurements `content_height`/`scroll_offset` need, then re-clamp: the window
        // may have been resized since the last frame, which changes both the wrap and the viewport.
        self.wrapped_len = lines.len();
        self.view_rows = rows;
        let max = self.max_scroll();
        if self.stick_bottom {
            self.scroll = max;
        } else if self.scroll > max {
            self.scroll = max;
        }

        let last_idx = lines.len().saturating_sub(1);

        for (row, idx) in (self.scroll..lines.len()).take(rows).enumerate() {
            let text = &lines[idx];
            let y = PAD_Y + row * TERM_LINE_H;
            let mut x = PAD_X;

            // `.p` is the prompt sigil, `.o` is the echoed command, everything else is `--fg-2`
            // output. Only a line that actually starts with the sigil gets the treatment, so a
            // wrapped continuation of a long command is not re-coloured halfway through.
            let (sigil, rest) = match text.strip_prefix("N> ") {
                Some(r) => ("N> ", r),
                None => ("", text.as_str()),
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

        if key == '\n' || key == '\r' {
            // Typing always returns you to the live end of the output — a command whose result you
            // could not see because the view was parked in history would be baffling.
            self.stick_bottom = true;
            // Own the command string so the dispatch below can take `&mut self` (do_compile /
            // list_toolchains) without colliding with a borrow of self.input_buffer.
            let cmd = self.input_buffer.trim().to_string();
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
                self.output_history.push_str("  links             - list the last page's links\n");
                self.output_history.push_str("  open <n>          - follow link <n>\n");
                self.output_history.push_str("  dns <host>        - resolve a name, with timing (says WHY a lookup failed)\n");
                self.output_history.push_str("  date              - the clock. A wrong clock makes EVERY https:// fail\n");
                self.output_history.push_str("  time sync [url]   - set the clock from a plain-http server's Date header\n");
                self.output_history.push_str("  tz [+5:30]        - DISPLAY timezone (clock itself stays UTC). India: tz +5:30\n");
                self.output_history.push_str("  date offset <s>   - apply a clock offset directly, no network (bisects time sync)\n");
                self.output_history.push_str("  fetch <url>       - raw GET: status, size, first 1 KB unrendered\n");
                self.output_history.push_str("  exec <path>       - fork+execve any binary (a bad one kills this window, not the desktop)\n");
                self.output_history.push_str("Hardware probes:\n");
                self.output_history.push_str("  panel             - what ACPI says about the backlight: _BCL levels and _BQC current (READ ONLY)\n");
                self.output_history.push_str("  panel set <5-100> - drive the panel via _BCM. On this laptop that ends in a firmware SMI\n");
                self.output_history.push_str("  acpi log          - everything ACPICA printed since boot. Read this FIRST when anything ACPI is missing\n");
                self.output_history.push_str("  battery           - ACPI control-method battery: charge, rate, health (READ ONLY)\n");
                self.output_history.push_str("Scrollback:\n");
                self.output_history.push_str("  PageUp / PageDown - page through history   Home / End - jump to top / live end\n");
                self.output_history.push_str("  (or drag the scrollbar; new output follows only when you are at the bottom)\n");
                self.output_history.push_str("Fatal-screen self-test (proves a crash can still report itself):\n");
                self.output_history.push_str("  fault segv        - child derefs null   -> amber banner, desktop survives\n");
                self.output_history.push_str("  fault ud          - child runs ud2      -> amber banner, desktop survives\n");
                self.output_history.push_str("  fault panic       - KILLS THE MACHINE   -> full red screen, then power cycle\n");
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
                        if !self.output_history.ends_with('\n') { self.output_history.push('\n'); }
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
                        Ok(s) if (1..=7).contains(&s) => {
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
                        //   all distinct           -> the tree is genuinely that wide
                        let mut seen: Vec<u64> = Vec::new();
                        let mut verdict = String::new();
                        while n < 512 {
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
                                    // Handle printed too: names can collide, handles cannot.
                                    self.output_history.push_str(&format!(
                                        "  {:<10} {:#012x}  {}\n",
                                        node.type_name(),
                                        node.handle,
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
