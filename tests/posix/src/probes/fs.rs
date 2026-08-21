//! Filesystem probes.
//!
//! These have an oracle that most kernels do not offer: Nyx's own syscalls 510/511/541 (directory
//! entry count, entry by index, file size) already work. So for every POSIX call here we know what
//! the right answer *is*, and a `FAIL` verdict means "disagrees with the oracle" rather than merely
//! "returned something odd".

use crate::raw;
use crate::report::{render_ret, render_std, Baseline, Row, Verdict};

use super::{announce, scratch, ASSET, KNOWN_DIR, SCRATCH};

/// Linux `open` flags, as the Nyx kernel understands them.
const O_RDONLY: usize = 0;
const O_WRONLY: usize = 1;
const O_CREAT: usize = 0o100;
const O_TRUNC: usize = 0o1000;

/// Nyx's `open` takes `(ptr, len, flags)` — a slice, NOT a NUL-terminated string. That divergence
/// is the first thing a musl port collides with, so the harness uses the kernel's real shape and
/// reports the difference rather than pretending.
unsafe fn nyx_open(path: &str, flags: usize) -> isize {
    unsafe { raw::sys3(raw::SYS_OPEN, path.as_ptr() as usize, path.len(), flags) }
}

/// **Path shape.**
///
/// Every path syscall here is `(ptr, len)` — a slice, not a NUL-terminated C string. That is Nyx's
/// convention: `open(2)` has always worked this way, and Phase 1 deliberately followed it for the
/// whole new fs block rather than introducing a second convention alongside it. Converting the
/// kernel to C strings is a single atomic change owed to the musl port.
///
/// This is a divergence from Linux and it is what a libc collides with first, so the `open` row
/// carries it as a visible note rather than leaving it to be rediscovered.
///
/// (Before these calls existed, the probes deliberately passed pointers in every slot *either*
/// convention could dereference, because a length landing in a pointer slot against a Linux-shaped
/// implementation would have read a string from address ~40 — and `is_valid_user_ptr` only
/// range-checks, so a kernel-mode fault there panics the machine. Now that the kernel side is
/// written rather than guessed at, the calls match it exactly.)

pub fn probe_readonly(base: Baseline, rows: &mut Vec<Row>) {
    // --- the oracle ------------------------------------------------------------------------
    announce("oracle (510/541)");
    let dir_count =
        unsafe { raw::sys2(raw::SYS_NYX_DIR_COUNT, KNOWN_DIR.as_ptr() as usize, KNOWN_DIR.len()) };
    let oracle_size =
        unsafe { raw::sys2(raw::SYS_NYX_FILE_SIZE, ASSET.as_ptr() as usize, ASSET.len()) };
    let mut row = Row::new(510, "nyx dir_count");
    row.raw = render_ret(dir_count);
    row.verdict = if dir_count > 0 { Verdict::Pass } else { Verdict::Fail };
    row.note = format!("oracle: {} entries in {KNOWN_DIR}", dir_count.max(0) as u32);
    rows.push(row);

    let mut row = Row::new(541, "nyx file_size");
    row.raw = render_ret(oracle_size);
    row.verdict = if oracle_size > 0 { Verdict::Pass } else { Verdict::Fail };
    row.note = format!("oracle: probe.txt is {} bytes", oracle_size.max(0) as u32);
    rows.push(row);

    // --- open / read -----------------------------------------------------------------------
    announce("open");
    let fd = unsafe { nyx_open(ASSET, O_RDONLY) };
    let mut row = Row::new(2, "open");
    row.raw = render_ret(fd);
    row.std_api = render_std(&std::fs::File::open(ASSET));
    row.verdict = classify(base, fd, None, &row.std_api);
    row.note = String::from("takes (ptr,len,flags), not a C string");
    rows.push(row);

    if fd < 0 {
        // Everything below needs a live fd; say so once instead of cascading failures.
        let mut row = Row::new(0, "fd-dependent");
        row.raw = String::from("-");
        row.verdict = Verdict::Skip;
        row.note = String::from("could not open the bundled probe.txt; is it installed?");
        rows.push(row);
        return;
    }
    let fd = fd as usize;

    announce("read");
    let mut buf = [0u8; 32];
    let n = unsafe { raw::sys3(raw::SYS_READ, fd, buf.as_mut_ptr() as usize, buf.len()) };
    let first_byte = if n > 0 { buf[0] } else { 0 };
    let mut row = Row::new(0, "read");
    row.raw = render_ret(n);
    row.verdict = if n > 0 { Verdict::Pass } else { Verdict::Fail };
    rows.push(row);

    // --- lseek: verified by re-reading a known byte, not by the return value ---------------
    announce("lseek");
    let seek_ret = unsafe { raw::sys3(raw::SYS_LSEEK, fd, 0, 0) }; // SEEK_SET to 0
    let mut again = [0u8; 1];
    let reread = unsafe { raw::sys3(raw::SYS_READ, fd, again.as_mut_ptr() as usize, 1) };
    let rewound = reread == 1 && again[0] == first_byte && n > 0;

    let mut row = Row::new(8, "lseek");
    row.raw = render_ret(seek_ret);
    // std::io::Seek is the PAL's own path and fails independently of the syscall.
    row.std_api = {
        use std::io::Seek;
        match std::fs::File::open(ASSET) {
            Ok(mut f) => render_std(&f.seek(std::io::SeekFrom::Start(0))),
            Err(e) => format!("open:{:?}", e.kind()),
        }
    };
    row.verdict = classify(base, seek_ret, Some(rewound), &row.std_api);
    if seek_ret >= 0 && !rewound {
        row.note = String::from("returned success but the file position did not move");
    }
    rows.push(row);

    // --- fstat -----------------------------------------------------------------------------
    announce("fstat");
    // struct stat is 144 bytes on x86_64; give the kernel a real, correctly-sized local.
    let mut statbuf = [0u8; 144];
    let r = unsafe { raw::sys2(raw::SYS_FSTAT, fd, statbuf.as_mut_ptr() as usize) };
    let mut row = Row::new(5, "fstat");
    row.raw = render_ret(r);
    row.std_api = render_std(&std::fs::metadata(ASSET));
    // A `stat` that succeeds and leaves the whole struct zero is the definition of a STUB.
    let filled = statbuf.iter().any(|&b| b != 0);
    row.verdict = classify(base, r, Some(filled), &row.std_api);
    if r >= 0 && !filled {
        row.note = String::from("succeeded but left struct stat entirely zero");
    }
    rows.push(row);

    unsafe { raw::sys1(raw::SYS_CLOSE, fd) };

    // --- stat / newfstatat -----------------------------------------------------------------
    announce("stat");
    let mut statbuf = [0u8; 144];
    let sb = statbuf.as_mut_ptr() as usize;
    let r = unsafe { raw::sys3(raw::SYS_STAT, ASSET.as_ptr() as usize, ASSET.len(), sb) };
    let mut row = Row::new(4, "stat");
    row.raw = render_ret(r);
    row.std_api = render_std(&std::fs::metadata(ASSET));
    row.verdict = classify(base, r, Some(statbuf.iter().any(|&b| b != 0)), &row.std_api);
    rows.push(row);

    announce("newfstatat");
    let mut statbuf = [0u8; 144];
    let r = unsafe {
        raw::sys4(
            raw::SYS_NEWFSTATAT,
            (-100isize) as usize, // AT_FDCWD — accepted and ignored until Process has a cwd
            // (Phase 4), but it is the value a libc actually passes, so it is what gets measured.
            ASSET.as_ptr() as usize,
            ASSET.len(),
            statbuf.as_mut_ptr() as usize,
        )
    };
    let mut row = Row::new(262, "newfstatat");
    row.raw = render_ret(r);
    row.verdict = simple(base, r);
    rows.push(row);

    // --- getdents64: cross-checked against the oracle's entry count -------------------------
    announce("getdents64");
    let dirfd = unsafe { nyx_open(KNOWN_DIR, O_RDONLY) };
    let mut dents = [0u8; 2048];
    let r = if dirfd >= 0 {
        unsafe {
            raw::sys3(raw::SYS_GETDENTS64, dirfd as usize, dents.as_mut_ptr() as usize, dents.len())
        }
    } else {
        base.0
    };
    if dirfd >= 0 {
        unsafe { raw::sys1(raw::SYS_CLOSE, dirfd as usize) };
    }
    let mut row = Row::new(217, "getdents64");
    row.raw = render_ret(r);
    row.std_api = match std::fs::read_dir(KNOWN_DIR) {
        Ok(it) => format!("ok({})", it.count() as u32),
        Err(e) => format!("{:?}", e.kind()),
    };
    // getdents64 returns the number of BYTES written, so 0 means "wrote nothing" — a directory the
    // oracle says has entries in it. That is a failure, not an empty directory.
    row.verdict = classify(base, r, Some(r > 0), &row.std_api);
    row.note = format!("oracle says {} entries", dir_count.max(0) as u32);
    rows.push(row);

    // --- access / dup / dup2 / fcntl / pipe2 -----------------------------------------------
    announce("access");
    let r = unsafe { raw::sys3(raw::SYS_ACCESS, ASSET.as_ptr() as usize, ASSET.len(), 0) };
    let mut row = Row::new(21, "access");
    row.raw = render_ret(r);
    row.std_api = format!("exists={}", std::path::Path::new(ASSET).exists());
    row.verdict = classify(base, r, None, &row.std_api);
    rows.push(row);

    announce("dup / dup2");
    let fd = unsafe { nyx_open(ASSET, O_RDONLY) };
    if fd >= 0 {
        let dup_ret = unsafe { raw::sys1(raw::SYS_DUP, fd as usize) };
        let mut row = Row::new(32, "dup");
        row.raw = render_ret(dup_ret);
        row.verdict = simple(base, dup_ret);
        rows.push(row);
        if dup_ret >= 0 {
            unsafe { raw::sys1(raw::SYS_CLOSE, dup_ret as usize) };
        }

        // dup2 onto a high, definitely-free descriptor rather than over anything live.
        let dup2_ret = unsafe { raw::sys2(raw::SYS_DUP2, fd as usize, 20) };
        let mut row = Row::new(33, "dup2");
        row.raw = render_ret(dup2_ret);
        row.verdict = simple(base, dup2_ret);
        rows.push(row);
        if dup2_ret >= 0 {
            unsafe { raw::sys1(raw::SYS_CLOSE, 20) };
        }

        announce("fcntl");
        let r = unsafe { raw::sys3(raw::SYS_FCNTL, fd as usize, 3, 0) }; // F_GETFL
        let mut row = Row::new(72, "fcntl");
        row.raw = render_ret(r);
        row.verdict = simple(base, r);
        rows.push(row);

        unsafe { raw::sys1(raw::SYS_CLOSE, fd as usize) };
    }

    announce("pipe2");
    let mut fds = [0i32; 2];
    let r = unsafe { raw::sys2(raw::SYS_PIPE2, fds.as_mut_ptr() as usize, 0) };
    let mut row = Row::new(293, "pipe2");
    row.raw = render_ret(r);
    row.verdict = simple(base, r);
    if r >= 0 {
        unsafe {
            raw::sys1(raw::SYS_CLOSE, fds[0] as usize);
            raw::sys1(raw::SYS_CLOSE, fds[1] as usize);
        }
    }
    rows.push(row);
}

pub fn probe_cwd(base: Baseline, rows: &mut Vec<Row>) {
    announce("getcwd");
    let mut buf = [0u8; 256];
    let r = unsafe { raw::sys2(raw::SYS_GETCWD, buf.as_mut_ptr() as usize, buf.len()) };
    let mut row = Row::new(79, "getcwd");
    row.raw = render_ret(r);
    row.std_api = match std::env::current_dir() {
        Ok(p) => format!("ok({})", p.display()),
        Err(e) => format!("{:?}", e.kind()),
    };
    row.verdict = classify(base, r, None, &row.std_api);
    row.note = String::from("Process has no cwd field yet");
    rows.push(row);

    announce("chdir");
    let target = KNOWN_DIR;
    let r = unsafe { raw::sys2(raw::SYS_CHDIR, target.as_ptr() as usize, target.len()) };
    let mut row = Row::new(80, "chdir");
    row.raw = render_ret(r);
    row.verdict = simple(base, r);
    rows.push(row);
}

/// Does a path exist, asked the cheapest honest way?
///
/// `access(2)` rather than `open(2)`: a symlink or a directory is not necessarily openable, and
/// "could not open it" is not the same claim as "it is not there".
fn exists(path: &str) -> bool {
    unsafe { raw::sys3(raw::SYS_ACCESS, path.as_ptr() as usize, path.len(), 0) >= 0 }
}

pub fn probe_destructive(base: Baseline, rows: &mut Vec<Row>) {
    // Everything below is confined to SCRATCH by `scratch()`, which panics on escape.
    //
    // The ORDER matters and is not arbitrary: each probe needs a live subject, and `rename` consumes
    // the file the round-trip created. So the chain is create → rename → link → unlink the renamed
    // file → rmdir, with every step verified by its effect rather than its return code.
    let dir = scratch("probe_dir");
    let file = scratch("probe_file");
    let renamed = scratch("probe_renamed");
    let link = scratch("probe_link");

    // --- open on a path that does not exist -------------------------------------------------
    // Its own row because nothing else asserts it, and the one thing that ever noticed reported it
    // as a broken `rename`. An open(2) that succeeds for any path makes every existence check
    // written as an open silently true, and a read of a missing file indistinguishable from an
    // empty one.
    announce("open (nonexistent)");
    let ghost = scratch("definitely_absent_file");
    let gfd = unsafe { nyx_open(&ghost, O_RDONLY) };
    if gfd >= 0 {
        unsafe { raw::sys1(raw::SYS_CLOSE, gfd as usize) };
    }
    let mut row = Row::new(2, "open (missing)");
    row.raw = render_ret(gfd);
    row.verdict = if gfd < 0 { Verdict::Pass } else { Verdict::Fail };
    row.note = String::from(if gfd < 0 {
        "correctly refuses a path that is not there"
    } else {
        "returned a descriptor for a file that does not exist"
    });
    rows.push(row);

    announce("mkdir");
    let r = unsafe { raw::sys3(raw::SYS_MKDIR, dir.as_ptr() as usize, dir.len(), 0o755) };
    let made = exists(&dir);
    let mut row = Row::new(83, "mkdir");
    row.raw = render_ret(r);
    row.std_api = render_std(&std::fs::create_dir(scratch("probe_dir_std")));
    row.verdict = classify(base, r, Some(made), &row.std_api);
    if r >= 0 && !made {
        row.note = String::from("returned success but no directory appeared");
    }
    rows.push(row);

    // --- write + read back: the round trip is the actual test ------------------------------
    announce("write/read round trip");
    let fd = unsafe { nyx_open(&file, O_WRONLY | O_CREAT | O_TRUNC) };
    let payload = b"nyx-posix-probe";
    let wrote = if fd >= 0 {
        let n = unsafe { raw::raw_write(fd as usize, payload) };
        unsafe { raw::sys1(raw::SYS_CLOSE, fd as usize) };
        n
    } else {
        fd
    };
    let mut readback = [0u8; 32];
    let rfd = unsafe { nyx_open(&file, O_RDONLY) };
    let got = if rfd >= 0 {
        let n = unsafe { raw::sys3(raw::SYS_READ, rfd as usize, readback.as_mut_ptr() as usize, readback.len()) };
        unsafe { raw::sys1(raw::SYS_CLOSE, rfd as usize) };
        n
    } else {
        rfd
    };
    let matched = got == payload.len() as isize && &readback[..payload.len()] == payload;
    let mut row = Row::new(0, "write→read");
    row.raw = format!("wrote {} read {}", wrote as i32, got as i32);
    row.verdict = if matched { Verdict::Pass } else { Verdict::Fail };
    if !matched {
        row.note = String::from("what came back was not what went in");
    }
    rows.push(row);

    announce("rename");
    let r = unsafe {
        raw::sys4(
            raw::SYS_RENAME,
            file.as_ptr() as usize,
            file.len(),
            renamed.as_ptr() as usize,
            renamed.len(),
        )
    };
    // Verified with access(2), the same way mkdir/symlink/unlink are checked. It used to use
    // open(2), which made this the ONLY row whose verdict depended on open failing for a missing
    // path — so an `open` that wrongly succeeds on a missing file would report a perfectly good
    // rename as a STUB, and send the reader to debug ext4 instead of open. Both signals are kept
    // below so the two causes stay distinguishable.
    let old_gone = !exists(&file);
    let new_there = exists(&renamed);
    let moved = old_gone && new_there;
    let old_open = unsafe { nyx_open(&file, O_RDONLY) };
    if old_open >= 0 {
        unsafe { raw::sys1(raw::SYS_CLOSE, old_open as usize) };
    }
    let mut row = Row::new(82, "rename");
    // The extra fields are the diagnosis: if `old_gone` is true but the old path still OPENS, the
    // bug is in open(2), not in rename.
    row.raw = format!(
        "{} old_gone={} new={} old_open={}",
        render_ret(r), old_gone as u8, new_there as u8, old_open as i32
    );
    // Give std a real subject rather than a missing one — renaming a nonexistent file would
    // exercise the error path and say nothing about whether the PAL can rename.
    row.std_api = {
        let a = scratch("probe_a_std");
        let b = scratch("probe_b_std");
        let _ = std::fs::write(&a, b"x");
        render_std(&std::fs::rename(&a, &b))
    };
    row.verdict = classify(base, r, Some(moved), &row.std_api);
    if r >= 0 && !moved {
        row.note = String::from("returned success but the file did not move");
    }
    rows.push(row);

    announce("symlink");
    let r = unsafe {
        raw::sys4(
            raw::SYS_SYMLINK,
            renamed.as_ptr() as usize,
            renamed.len(),
            link.as_ptr() as usize,
            link.len(),
        )
    };
    let linked = exists(&link);
    let mut row = Row::new(88, "symlink");
    row.raw = render_ret(r);
    row.verdict = classify(base, r, Some(linked), "-");
    if r >= 0 && !linked {
        row.note = String::from("returned success but no link appeared");
    } else {
        row.note = String::from("creating works; there is no readlink to read it back");
    }
    rows.push(row);

    // --- unlink: verified by the path going away -------------------------------------------
    // Target is `renamed`, not `file` — after a working rename, `file` no longer exists and
    // unlinking it would measure ENOENT rather than unlink.
    announce("unlink");
    let r = unsafe { raw::sys2(raw::SYS_UNLINK, renamed.as_ptr() as usize, renamed.len()) };
    let still_there = exists(&renamed);
    let mut row = Row::new(87, "unlink");
    row.raw = render_ret(r);
    row.std_api = {
        let p = scratch("probe_file_std");
        let _ = std::fs::write(&p, b"x");
        render_std(&std::fs::remove_file(&p))
    };
    row.verdict = classify(base, r, Some(!still_there), &row.std_api);
    if r >= 0 && still_there {
        row.note = String::from("returned success but the path is still there");
    }
    rows.push(row);

    announce("rmdir");
    let r = unsafe { raw::sys2(raw::SYS_RMDIR, dir.as_ptr() as usize, dir.len()) };
    let gone = !exists(&dir);
    let mut row = Row::new(84, "rmdir");
    row.raw = render_ret(r);
    row.std_api = render_std(&std::fs::remove_dir(scratch("probe_dir_std")));
    row.verdict = classify(base, r, Some(gone), &row.std_api);
    if r >= 0 && !gone {
        row.note = String::from("returned success but the directory is still there");
    }
    rows.push(row);

    // --- readdir: the PAL's own path, which is a separate wall from getdents64 --------------
    announce("std read_dir");
    let mut row = Row::new(0, "std read_dir");
    row.raw = String::from("-");
    row.std_api = match std::fs::read_dir(KNOWN_DIR) {
        Ok(it) => format!("ok({})", it.count() as u32),
        Err(e) => format!("{:?}", e.kind()),
    };
    row.verdict = if row.std_api.starts_with("ok(") { Verdict::Pass } else { Verdict::PalGap };
    row.note = String::from("std::fs::read_dir over getdents64");
    rows.push(row);

    // Idempotence: leave nothing behind, so a re-run from the UI produces an identical summary.
    // Every one of these may legitimately be absent — the probes above are supposed to have
    // removed them — so failures are ignored rather than reported.
    for leftover in [&file, &renamed, &link, &scratch("probe_a_std"), &scratch("probe_b_std"), &scratch("probe_file_std")] {
        let _ = std::fs::remove_file(leftover);
    }
    let _ = std::fs::remove_dir(&dir);
    let _ = std::fs::remove_dir(scratch("probe_dir_std"));
    println!("posix: scratch cleaned ({SCRATCH})");
}

/// Classify a probe that has no std equivalent to compare against.
fn simple(base: Baseline, ret: isize) -> Verdict {
    classify(base, ret, None, "-")
}

/// The one classifier every row goes through.
///
/// `effect_ok` is `Some(false)` when the call returned success and the observable effect did **not**
/// happen — that is a STUB, and it outranks everything below it. `None` means the row has no
/// side-effect test, so success is taken at face value.
///
/// **`PAL-GAP` is the reason this function exists**, and it is why every row with a STD column must
/// come through here rather than through a return-code check. A working kernel syscall sitting
/// behind a std API that still answers `Unsupported` is a different job from a missing syscall — it
/// is PAL work in `vendor/nyx-std/`, not kernel work — and it is invisible unless the two columns
/// are compared. The first cut of this harness wired the comparison to a single row and shipped a
/// `palgap=0` that was an artefact of the wiring, not a measurement.
fn classify(base: Baseline, ret: isize, effect_ok: Option<bool>, std_api: &str) -> Verdict {
    if base.is_missing(ret) {
        return Verdict::Missing;
    }
    if ret < 0 {
        return Verdict::Fail;
    }
    if effect_ok == Some(false) {
        return Verdict::Stub;
    }
    // `Unsupported` is what the PAL's `unsupported()` helper produces; `Uncategorized` is what the
    // old lying `generic.rs` stub produced for everything, and it means the same thing here — std
    // could not answer a question the kernel just answered.
    if std_api.contains("Unsupported") || std_api.contains("Uncategorized") {
        return Verdict::PalGap;
    }
    Verdict::Pass
}
