//! Event-demultiplexing probes.
//!
//! Nyx has no `poll`, `select` or `epoll` at all today, which is the single biggest hole for any
//! libc port — it is the spine of every C event loop, including Ladybird's `LibCore::EventLoop`.
//!
//! The test is behavioural, not a return-code check: a pipe with nothing in it must report *not*
//! ready, and the same pipe after a byte is written must report readable. A `poll` that always
//! claims readiness is worse than none, because it turns every event loop into a spin loop.

use crate::raw;
use crate::report::{render_ret, Baseline, Row, Verdict};

use super::announce;

const POLLIN: i16 = 0x001;
const POLLHUP: i16 = 0x010;

#[repr(C)]
#[derive(Clone, Copy)]
struct PollFd {
    fd: i32,
    events: i16,
    revents: i16,
}

pub fn probe(base: Baseline, rows: &mut Vec<Row>) {
    // A pipe gives us a descriptor whose readiness we control exactly.
    let mut fds = [0i32; 2];
    let piped = unsafe { raw::sys1(raw::SYS_PIPE, fds.as_mut_ptr() as usize) };
    if piped < 0 {
        let mut row = Row::new(7, "poll");
        row.raw = String::from("-");
        row.verdict = Verdict::Skip;
        row.note = String::from("could not create a pipe to test readiness against");
        rows.push(row);
        return;
    }
    let (rfd, wfd) = (fds[0], fds[1]);

    // --- poll ------------------------------------------------------------------------------
    announce("poll (empty pipe)");
    let mut pfd = PollFd { fd: rfd, events: POLLIN, revents: 0 };
    // timeout 0 = return immediately. Never block here: if poll is half-implemented, an infinite
    // timeout would hang the harness with no way out.
    let empty = unsafe { raw::sys3(raw::SYS_POLL, &mut pfd as *mut PollFd as usize, 1, 0) };
    let empty_revents = pfd.revents;

    announce("poll (pipe with data)");
    unsafe { raw::raw_write(wfd as usize, b"x") };
    let mut pfd = PollFd { fd: rfd, events: POLLIN, revents: 0 };
    let ready = unsafe { raw::sys3(raw::SYS_POLL, &mut pfd as *mut PollFd as usize, 1, 0) };
    let ready_revents = pfd.revents;

    let mut row = Row::new(7, "poll");
    row.raw = format!("empty={} data={}", empty as i32, ready as i32);
    row.verdict = if base.is_missing(empty) {
        Verdict::Missing
    } else if empty < 0 || ready < 0 {
        Verdict::Fail
    } else if empty == 0 && ready == 1 && (ready_revents & POLLIN) != 0 {
        Verdict::Pass
    } else {
        Verdict::Stub
    };
    if row.verdict == Verdict::Stub {
        row.note = format!(
            "readiness not tracked (revents empty={} data={})",
            empty_revents as i32, ready_revents as i32
        );
    }
    rows.push(row);

    // Drain so select sees the same starting conditions poll did.
    let mut sink = [0u8; 1];
    unsafe { raw::sys3(raw::SYS_READ, rfd as usize, sink.as_mut_ptr() as usize, 1) };

    // --- select ----------------------------------------------------------------------------
    announce("select");
    // fd_set is a 1024-bit array; a live, correctly-sized local (the harness never hands the
    // kernel a pointer it cannot dereference).
    let mut readfds = [0u64; 16];
    let idx = (rfd as usize) / 64;
    let bit = (rfd as usize) % 64;
    readfds[idx] |= 1u64 << bit;
    // Two things are deliberate here. A NULL timeout means "block until something is ready", which on
    // an empty pipe would hang the harness forever the day select is implemented — so a real, zeroed
    // `timeval` makes this always a poll. And the write/except sets are live zeroed arrays rather
    // than NULL: `is_valid_user_ptr` accepts NULL, so an implementation that reads before checking
    // for it would fault in kernel mode and panic the machine. An empty set means the same thing.
    let mut writefds = [0u64; 16];
    let mut exceptfds = [0u64; 16];
    let timeout: [i64; 2] = [0, 0];
    let r = unsafe {
        raw::syscall6(
            raw::SYS_SELECT,
            (rfd + 1) as usize,
            readfds.as_mut_ptr() as usize,
            writefds.as_mut_ptr() as usize,
            exceptfds.as_mut_ptr() as usize,
            timeout.as_ptr() as usize,
            0,
        )
    };
    let mut row = Row::new(23, "select");
    row.raw = render_ret(r);
    row.verdict = if base.is_missing(r) { Verdict::Missing } else if r < 0 { Verdict::Fail } else { Verdict::Pass };
    rows.push(row);

    // --- HUP: a pipe whose writer has gone must say so --------------------------------------
    //
    // This needs its own row because the rows above cannot reach it: they keep the write end open
    // throughout, so an implementation that never reports POLLHUP still passes them both. That is
    // the same trap as the old `palgap=0` — a verdict no code path can produce is not a
    // measurement. A reader that cannot see EOF blocks forever on a closed pipe.
    announce("poll (writer closed)");
    let mut hfds = [0i32; 2];
    let mut row = Row::new(7, "poll HUP");
    if unsafe { raw::sys1(raw::SYS_PIPE, hfds.as_mut_ptr() as usize) } < 0 {
        row.raw = String::from("-");
        row.verdict = Verdict::Skip;
        row.note = String::from("could not create a pipe to hang up");
    } else {
        // Close the write end and nothing else: the read end must now report hangup.
        unsafe { raw::sys1(raw::SYS_CLOSE, hfds[1] as usize) };
        let mut pfd = PollFd { fd: hfds[0], events: POLLIN, revents: 0 };
        let r = unsafe { raw::sys3(raw::SYS_POLL, &mut pfd as *mut PollFd as usize, 1, 0) };
        row.raw = format!("ret={} revents={}", r as i32, pfd.revents as i32);
        row.verdict = if base.is_missing(r) {
            Verdict::Missing
        } else if r < 0 {
            Verdict::Fail
        } else if (pfd.revents & POLLHUP) != 0 {
            Verdict::Pass
        } else {
            Verdict::Stub
        };
        if row.verdict == Verdict::Stub {
            row.note = String::from("writer closed but no POLLHUP — a reader would hang on EOF");
        }
        unsafe { raw::sys1(raw::SYS_CLOSE, hfds[0] as usize) };
    }
    rows.push(row);

    // --- epoll family: presence only -------------------------------------------------------
    //
    // Every pointer slot gets a live buffer, never 0. `epoll_ctl` dereferences its event struct and
    // `epoll_wait` writes into its array, and a NULL there passes `is_valid_user_ptr` and then
    // faults in kernel mode — which panics the machine rather than this process. The timeout is 0 so
    // a real `epoll_wait` returns immediately instead of parking the harness.
    let mut evbuf = [0u8; 256];
    let ev = evbuf.as_mut_ptr() as usize;
    for (num, name, a1, a2, a3, a4) in [
        (raw::SYS_EPOLL_CREATE, "epoll_create", 1usize, 0usize, 0usize, 0usize),
        (raw::SYS_EPOLL_CREATE1, "epoll_create1", 0, 0, 0, 0),
        (raw::SYS_EPOLL_CTL, "epoll_ctl", 1, 1, rfd as usize, ev),
        (raw::SYS_EPOLL_WAIT, "epoll_wait", 1, ev, 1, 0),
        (raw::SYS_EPOLL_PWAIT, "epoll_pwait", 1, ev, 1, 0),
    ] {
        announce(name);
        let r = unsafe { raw::sys4(num, a1, a2, a3, a4) };
        let mut row = Row::new(num as u32, name);
        row.raw = render_ret(r);
        row.verdict = if base.is_missing(r) { Verdict::Missing } else { Verdict::Skip };
        if !base.is_missing(r) {
            row.note = String::from("present, not exercised");
        }
        rows.push(row);
    }

    unsafe {
        raw::sys1(raw::SYS_CLOSE, rfd as usize);
        raw::sys1(raw::SYS_CLOSE, wfd as usize);
    }
}
