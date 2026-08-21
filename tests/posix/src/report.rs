//! Verdicts, rows, and the serial log format.

use crate::raw;

#[derive(Clone, Copy, PartialEq, Eq)]
pub enum Verdict {
    /// Called, and the observable **effect** was verified — not merely a non-negative return.
    Pass,
    /// Present, but wrong: it claimed success and produced the wrong answer.
    Fail,
    /// Returned success and demonstrably changed nothing. The most dangerous state, because
    /// everything above it believes the operation happened.
    Stub,
    /// Return value identical to the unknown-syscall baseline.
    Missing,
    /// The kernel has it, but the Rust std PAL does not expose it.
    PalGap,
    /// Precondition unmet (no network link, no scratch mount, ring-0 only).
    Skip,
}

impl Verdict {
    pub fn label(self) -> &'static str {
        match self {
            Verdict::Pass => "PASS",
            Verdict::Fail => "FAIL",
            Verdict::Stub => "STUB",
            Verdict::Missing => "MISSING",
            Verdict::PalGap => "PAL-GAP",
            Verdict::Skip => "SKIP",
        }
    }

    pub fn color(self) -> u32 {
        match self {
            Verdict::Pass => 0xFF_27AE60,
            Verdict::Fail => 0xFF_C0392B,
            Verdict::Stub => 0xFF_E67E22,
            Verdict::Missing => 0xFF_8A8A85,
            Verdict::PalGap => 0xFF_8E44AD,
            Verdict::Skip => 0xFF_B0B0AA,
        }
    }
}

pub struct Row {
    /// Syscall number, or 0 for rows that are not a single syscall.
    pub num: u32,
    pub name: String,
    /// What the raw syscall returned, rendered.
    pub raw: String,
    /// What the equivalent std API did, or "-" when std has no equivalent.
    pub std_api: String,
    pub verdict: Verdict,
    /// Why this verdict, when the numbers alone do not say it.
    pub note: String,
}

impl Row {
    pub fn new(num: u32, name: &str) -> Row {
        Row {
            num,
            name: name.to_string(),
            raw: String::from("-"),
            std_api: String::from("-"),
            verdict: Verdict::Skip,
            note: String::new(),
        }
    }
}

/// This kernel's answer for a syscall that does not exist.
///
/// Nyx's dispatcher currently ends in `_ => { frame.rax = EINVAL }`, so "unimplemented" and "bad
/// argument" are the same value. Rather than hard-code either, the harness asks the kernel once per
/// run and compares every later probe against the answer. That keeps this tool correct both before
/// and after that arm is changed to ENOSYS — and the printed baseline says which world you are in.
#[derive(Clone, Copy)]
pub struct Baseline(pub isize);

impl Baseline {
    pub fn measure() -> Baseline {
        Baseline(unsafe { raw::sys0(raw::SYS_NEVER) })
    }

    pub fn is_missing(self, ret: isize) -> bool {
        ret == self.0
    }
}

/// Render a syscall return for the RAW column.
pub fn render_ret(ret: isize) -> String {
    if ret >= 0 {
        // Cast to u32: `tests/stdhello` carries a whole in-code fmt self-test because u64/usize
        // interpolation rendered as tofu in the boot log. Do not re-discover that here.
        format!("ok({})", ret as u32)
    } else {
        format!("{}({})", ret as i32, raw::errno_name(ret))
    }
}

/// Render an `io::Result` for the STD column.
pub fn render_std<T>(r: &std::io::Result<T>) -> String {
    match r {
        Ok(_) => String::from("ok"),
        Err(e) => match e.raw_os_error() {
            Some(code) => format!("{:?}({})", e.kind(), code as u32),
            None => format!("{:?}", e.kind()),
        },
    }
}

pub struct Report {
    pub rows: Vec<Row>,
    pub baseline: Baseline,
}

impl Report {
    pub fn count(&self, v: Verdict) -> usize {
        self.rows.iter().filter(|r| r.verdict == v).count()
    }

    /// One line per row, greppable and diffable between runs.
    pub fn log(&self) {
        println!("posix: baseline for an unknown syscall = {}", render_ret(self.baseline.0));
        for row in &self.rows {
            let mut line = String::new();
            if row.num > 0 {
                line.push_str(&format!("posix: {:04} ", row.num));
            } else {
                line.push_str("posix:      ");
            }
            line.push_str(&format!(
                "{:<16} RAW={:<18} STD={:<22} VERDICT={}",
                row.name,
                row.raw,
                row.std_api,
                row.verdict.label()
            ));
            if !row.note.is_empty() {
                line.push_str(&format!("  ({})", row.note));
            }
            println!("{line}");
        }
        println!(
            "posix: SUMMARY pass={} fail={} stub={} missing={} palgap={} skip={} baseline={}",
            self.count(Verdict::Pass) as u32,
            self.count(Verdict::Fail) as u32,
            self.count(Verdict::Stub) as u32,
            self.count(Verdict::Missing) as u32,
            self.count(Verdict::PalGap) as u32,
            self.count(Verdict::Skip) as u32,
            self.baseline.0 as i32,
        );
    }
}
