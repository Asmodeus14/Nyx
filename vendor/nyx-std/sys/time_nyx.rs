//! Nyx time — `Instant` (CLOCK_MONOTONIC) and `SystemTime` (CLOCK_REALTIME) over the kernel's
//! `SYS_CLOCK_GETTIME (228)`. Wired in via a `nyx` arm in `sys/time/mod.rs` (see apply.py). The
//! public interface mirrors `sys/time/unsupported.rs`: both types wrap a `Duration`.
#![allow(unsafe_op_in_unsafe_fn)]

use crate::time::Duration;

const SYS_CLOCK_GETTIME: usize = 228;
const CLOCK_REALTIME: usize = 0;
const CLOCK_MONOTONIC: usize = 1;

#[inline]
fn now(clockid: usize) -> Duration {
    let mut ts: [i64; 2] = [0, 0]; // {tv_sec, tv_nsec}
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") SYS_CLOCK_GETTIME => _,
            in("rdi") clockid,
            in("rsi") ts.as_mut_ptr(),
            out("rcx") _, out("r11") _,
            options(nostack),
        );
    }
    Duration::new(ts[0].max(0) as u64, ts[1].clamp(0, 999_999_999) as u32)
}

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct Instant(Duration);

#[derive(Copy, Clone, PartialEq, Eq, PartialOrd, Ord, Debug, Hash)]
pub struct SystemTime(Duration);

pub const UNIX_EPOCH: SystemTime = SystemTime(Duration::from_secs(0));

impl Instant {
    pub fn now() -> Instant {
        Instant(now(CLOCK_MONOTONIC))
    }

    pub fn checked_sub_instant(&self, other: &Instant) -> Option<Duration> {
        self.0.checked_sub(other.0)
    }

    pub fn checked_add_duration(&self, other: &Duration) -> Option<Instant> {
        Some(Instant(self.0.checked_add(*other)?))
    }

    pub fn checked_sub_duration(&self, other: &Duration) -> Option<Instant> {
        Some(Instant(self.0.checked_sub(*other)?))
    }
}

impl SystemTime {
    pub const MAX: SystemTime = SystemTime(Duration::MAX);
    pub const MIN: SystemTime = SystemTime(Duration::ZERO);

    pub fn now() -> SystemTime {
        SystemTime(now(CLOCK_REALTIME))
    }

    pub fn sub_time(&self, other: &SystemTime) -> Result<Duration, Duration> {
        self.0.checked_sub(other.0).ok_or_else(|| other.0 - self.0)
    }

    pub fn checked_add_duration(&self, other: &Duration) -> Option<SystemTime> {
        Some(SystemTime(self.0.checked_add(*other)?))
    }

    pub fn checked_sub_duration(&self, other: &Duration) -> Option<SystemTime> {
        Some(SystemTime(self.0.checked_sub(*other)?))
    }
}
