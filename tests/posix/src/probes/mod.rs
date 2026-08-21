//! The probe blocks, ordered safest → most destructive.

pub mod fs;
pub mod mem;
pub mod misc;
pub mod poll;
pub mod signals;

use crate::report::{Baseline, Report, Row};

/// Where every destructive probe is allowed to operate, and nowhere else.
pub const SCRATCH: &str = "/mnt/nvme/apps/PosixProbe.nyx/scratch";
/// A file the build bundles next to the binary, used for read-only probes.
pub const ASSET: &str = "/mnt/nvme/apps/PosixProbe.nyx/probe.txt";
/// A directory that certainly exists and is small — the getdents/readdir target.
pub const KNOWN_DIR: &str = "/mnt/nvme/apps";

/// Build a path inside the scratch directory.
///
/// Panics if the result escapes `SCRATCH`. This looks paranoid for a string concat, but it is the
/// single guard standing between a typo in a probe and `unlink` being pointed at the app bundle.
pub fn scratch(name: &str) -> String {
    let path = format!("{SCRATCH}/{name}");
    assert!(
        path.starts_with(SCRATCH) && !name.contains("..") && !name.starts_with('/'),
        "destructive probe escaped the scratch directory: {path}"
    );
    path
}

/// Announce a probe on serial *before* running it, so a hard hang is attributable from the log
/// alone. Without this a freeze tells you only that the harness stopped, not where.
pub fn announce(name: &str) {
    println!("posix: >> {name}");
}

pub fn run_all() -> Report {
    let baseline = Baseline::measure();
    println!(
        "posix: ---- run start ---- unknown-syscall baseline = {}",
        crate::report::render_ret(baseline.0)
    );

    let mut rows: Vec<Row> = Vec::new();
    misc::probe(baseline, &mut rows);
    fs::probe_readonly(baseline, &mut rows);
    fs::probe_cwd(baseline, &mut rows);
    poll::probe(baseline, &mut rows);
    fs::probe_destructive(baseline, &mut rows);
    mem::probe(baseline, &mut rows);
    signals::probe(baseline, &mut rows);
    misc::probe_presence_only(baseline, &mut rows);

    let report = Report { rows, baseline };
    report.log();
    report
}
