//! An in-memory filesystem, mounted at `/tmp`.
//!
//! Phase 6, gate 3. `std::filesystem::temp_directory_path()` returns `/tmp` and every C++ library
//! that needs scratch space goes there; Nyx had no `/tmp` at all, so those calls failed against a
//! path that simply did not exist. (That absence is also what made `cxxtest` fall back to `/tmp`
//! and report four filesystem failures for a reason unrelated to the bug being hunted.)
//!
//! In memory rather than a directory on the ext4 volume, deliberately: temp files are written and
//! deleted constantly by a browser, and pushing that churn through the NVMe would both wear the
//! device and make every temp write pay disk latency. It also gets the semantics right — `/tmp`
//! is supposed to be empty after a reboot, and a directory on ext4 would silently accumulate
//! whatever the last run left behind.
//!
//! ★★★ CAPPED, and the cap is a safety mechanism rather than tidiness. This lives in the KERNEL
//! HEAP, which is 64 MiB total (`allocator::HEAP_SIZE`) and shared with task structures, network
//! buffers, GPU state and the ext4 caches. An unbounded tmpfs lets a userspace process exhaust the
//! kernel's heap by writing a large file — and an allocation failure in the kernel panics, which
//! since P1a paints to a framebuffer nobody is looking at. So a runaway `/tmp` would present as a
//! silent freeze with no output. `ENOSPC` is a far better outcome than that, and it is the answer
//! POSIX already defines for this.
//!
//! No new lock: the whole driver lives inside the `Box<dyn FileSystem>` held under `VFS.mounts`,
//! so every entry point is already serialised by the mount lock that all the other drivers take.
//! Adding a second lock here would create a new lock-ordering pair reachable from the syscall path,
//! which is exactly the shape of the deadlock documented in the syscall/lock discipline notes.

use alloc::collections::BTreeMap;
use alloc::string::String;
use alloc::vec::Vec;

use crate::vfs::{FileStat, FileSystem, FsError, StatFs, S_IFDIR, S_IFREG};

/// 8 MiB of a 64 MiB kernel heap. Deliberately conservative: this is a hard ceiling on how much
/// of the kernel's own memory unprivileged code can consume by writing files.
const TMPFS_CAP: usize = 8 * 1024 * 1024;

enum Node {
    File(Vec<u8>),
    Dir,
}

pub struct TmpFs {
    nodes: BTreeMap<String, Node>,
    inos: BTreeMap<String, u32>,
    used: usize,
    next_ino: u32,
}

/// Days from 1970-01-01 for a civil date. Howard Hinnant's `days_from_civil`, which is exact for
/// the whole proleptic Gregorian range and needs no lookup tables or leap-year special cases.
fn days_from_civil(y: i64, m: i64, d: i64) -> i64 {
    let y = if m <= 2 { y - 1 } else { y };
    let era = if y >= 0 { y } else { y - 399 } / 400;
    let yoe = y - era * 400;
    let mp = (m + 9) % 12;
    let doy = (153 * mp + 2) / 5 + d - 1;
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;
    era * 146097 + doe - 719468
}

/// Wall-clock seconds since the epoch, from the battery-backed RTC.
///
/// A real timestamp rather than a synthesised one, for the reason `FileStat` already documents:
/// anything that caches on mtime misbehaves against a clock that reports the same value for every
/// file. Uptime would have been easier and would have been a lie in a different direction — it
/// looks like an epoch but sits in 1970.
fn now_epoch() -> u32 {
    let packed = crate::rtc::read_packed();
    let (year, month, day) = (
        ((packed >> 40) & 0xFFFF) as i64,
        ((packed >> 32) & 0xFF) as i64,
        ((packed >> 24) & 0xFF) as i64,
    );
    let (hour, min, sec) = (
        ((packed >> 16) & 0xFF) as i64,
        ((packed >> 8) & 0xFF) as i64,
        (packed & 0xFF) as i64,
    );
    // A zero/garbage RTC reading would otherwise land in year 0 and produce a huge negative
    // number that wraps to something absurd when cast.
    if year < 1970 || month < 1 || month > 12 || day < 1 || day > 31 {
        return 0;
    }
    let days = days_from_civil(year, month, day);
    let secs = days * 86400 + hour * 3600 + min * 60 + sec;
    if secs < 0 { 0 } else { secs as u32 }
}

impl TmpFs {
    pub fn new() -> Self {
        let mut nodes = BTreeMap::new();
        // The mount point itself must exist, or the very first `stat("/tmp")` fails and every
        // library that checks for its temp directory before using it gives up.
        nodes.insert(String::from("/"), Node::Dir);
        let mut inos = BTreeMap::new();
        inos.insert(String::from("/"), 1u32);
        Self { nodes, inos, used: 0, next_ino: 2 }
    }

    /// Normalise to a leading-slash, no-trailing-slash key. `resolve_mount` already hands us a
    /// path starting with `/`, but "/tmp/x/" and "/tmp/x" must not become two different files.
    fn key(path: &str) -> String {
        let p = path.trim_end_matches('/');
        if p.is_empty() { String::from("/") } else { String::from(p) }
    }

    fn parent_of(key: &str) -> String {
        match key.rfind('/') {
            Some(0) | None => String::from("/"),
            Some(i) => String::from(&key[..i]),
        }
    }

    fn parent_is_dir(&self, key: &str) -> bool {
        matches!(self.nodes.get(&Self::parent_of(key)), Some(Node::Dir))
    }

    fn ino_for(&mut self, key: &str) -> u32 {
        if let Some(i) = self.inos.get(key) { return *i; }
        let i = self.next_ino;
        self.next_ino = self.next_ino.wrapping_add(1).max(2);
        self.inos.insert(String::from(key), i);
        i
    }
}

impl FileSystem for TmpFs {
    fn read_file(&self, path: &str, offset: usize, buf: &mut [u8]) -> Result<usize, FsError> {
        match self.nodes.get(&Self::key(path)) {
            Some(Node::File(data)) => {
                if offset >= data.len() { return Ok(0); } // EOF, not an error
                let n = core::cmp::min(buf.len(), data.len() - offset);
                buf[..n].copy_from_slice(&data[offset..offset + n]);
                Ok(n)
            }
            Some(Node::Dir) => Err(FsError::InvalidPath),
            None => Err(FsError::NotFound),
        }
    }

    fn write_file(&mut self, path: &str, offset: usize, buf: &[u8]) -> Result<usize, FsError> {
        let k = Self::key(path);
        if !self.nodes.contains_key(&k) {
            if !self.parent_is_dir(&k) { return Err(FsError::NotFound); }
            self.nodes.insert(k.clone(), Node::File(Vec::new()));
            self.ino_for(&k);
        }
        let end = offset.checked_add(buf.len()).ok_or(FsError::InvalidPath)?;
        // ★ Charge the cap BEFORE allocating, and charge only the GROWTH. Checking after the
        // Vec has already grown would be a cap that stops nothing: the allocation that exhausts
        // the heap is the one that has to be refused.
        let grew = {
            let cur = match self.nodes.get(&k) {
                Some(Node::File(d)) => d.len(),
                Some(Node::Dir) => return Err(FsError::PermissionDenied),
                None => return Err(FsError::NotFound),
            };
            end.saturating_sub(cur)
        };
        if self.used + grew > TMPFS_CAP { return Err(FsError::OutOfSpace); }

        match self.nodes.get_mut(&k) {
            Some(Node::File(data)) => {
                if end > data.len() {
                    // Writing past EOF zero-fills the hole, which is what POSIX specifies and what
                    // anything seeking then writing (a sparse cache file) depends on.
                    data.resize(end, 0);
                }
                data[offset..end].copy_from_slice(buf);
                self.used += grew;
                Ok(buf.len())
            }
            _ => Err(FsError::NotFound),
        }
    }

    fn get_file_size(&self, path: &str) -> Result<usize, FsError> {
        match self.nodes.get(&Self::key(path)) {
            Some(Node::File(d)) => Ok(d.len()),
            Some(Node::Dir) => Ok(0),
            None => Err(FsError::NotFound),
        }
    }

    fn create_file(&mut self, path: &str) -> Result<(), FsError> {
        let k = Self::key(path);
        match self.nodes.get(&k) {
            // Already a file: succeed. `open(O_CREAT)` on an existing file is not an error, and
            // the dispatcher calls this unconditionally on that path.
            Some(Node::File(_)) => Ok(()),
            Some(Node::Dir) => Err(FsError::PermissionDenied),
            None => {
                if !self.parent_is_dir(&k) { return Err(FsError::NotFound); }
                self.nodes.insert(k.clone(), Node::File(Vec::new()));
                self.ino_for(&k);
                Ok(())
            }
        }
    }

    fn create_dir(&mut self, path: &str) -> Result<(), FsError> {
        let k = Self::key(path);
        if self.nodes.contains_key(&k) { return Err(FsError::PermissionDenied); }
        if !self.parent_is_dir(&k) { return Err(FsError::NotFound); }
        self.nodes.insert(k.clone(), Node::Dir);
        self.ino_for(&k);
        Ok(())
    }

    fn list_dir(&self, path: &str) -> Result<Vec<String>, FsError> {
        let k = Self::key(path);
        match self.nodes.get(&k) {
            Some(Node::Dir) => {}
            Some(Node::File(_)) => return Err(FsError::InvalidPath),
            None => return Err(FsError::NotFound),
        }
        let prefix = if k == "/" { String::from("/") } else { alloc::format!("{}/", k) };
        let mut out = Vec::new();
        for name in self.nodes.keys() {
            if name.as_str() == k { continue; }
            if let Some(rest) = name.strip_prefix(prefix.as_str()) {
                // Immediate children only — a nested path would otherwise appear in its
                // grandparent's listing and make directory_iterator report the tree flattened.
                if !rest.is_empty() && !rest.contains('/') {
                    out.push(String::from(rest));
                }
            }
        }
        Ok(out)
    }

    fn delete_file(&mut self, path: &str) -> Result<(), FsError> {
        let k = Self::key(path);
        match self.nodes.get(&k) {
            Some(Node::File(d)) => {
                let freed = d.len();
                self.nodes.remove(&k);
                self.inos.remove(&k);
                self.used = self.used.saturating_sub(freed);
                Ok(())
            }
            Some(Node::Dir) => Err(FsError::PermissionDenied),
            None => Err(FsError::NotFound),
        }
    }

    fn remove_dir(&mut self, path: &str) -> Result<(), FsError> {
        let k = Self::key(path);
        if k == "/" { return Err(FsError::PermissionDenied); } // never unmount ourselves
        match self.nodes.get(&k) {
            Some(Node::Dir) => {}
            Some(Node::File(_)) => return Err(FsError::InvalidPath),
            None => return Err(FsError::NotFound),
        }
        if !self.list_dir(path)?.is_empty() { return Err(FsError::PermissionDenied); }
        self.nodes.remove(&k);
        self.inos.remove(&k);
        Ok(())
    }

    fn rename(&mut self, from: &str, to: &str) -> Result<(), FsError> {
        let (a, b) = (Self::key(from), Self::key(to));
        if a == "/" { return Err(FsError::PermissionDenied); }
        let node = self.nodes.remove(&a).ok_or(FsError::NotFound)?;
        if !self.parent_is_dir(&b) {
            self.nodes.insert(a, node); // put it back rather than losing the file
            return Err(FsError::NotFound);
        }
        // Replacing an existing destination file is what rename(2) does; reclaim its bytes.
        if let Some(Node::File(old)) = self.nodes.get(&b) {
            self.used = self.used.saturating_sub(old.len());
        }
        self.nodes.insert(b.clone(), node);
        if let Some(i) = self.inos.remove(&a) { self.inos.insert(b, i); }
        Ok(())
    }

    fn stat(&self, path: &str) -> Result<FileStat, FsError> {
        let k = Self::key(path);
        let (mode, size) = match self.nodes.get(&k) {
            Some(Node::Dir) => (S_IFDIR | 0o755, 0u64),
            Some(Node::File(d)) => (S_IFREG | 0o644, d.len() as u64),
            None => return Err(FsError::NotFound),
        };
        let t = now_epoch();
        Ok(FileStat {
            size,
            mode,
            // `ino` is looked up read-only here, so an unwritten node reports 0 rather than
            // allocating one from a &self method. Files get a real, stable ino on creation.
            ino: self.inos.get(&k).copied().unwrap_or(0),
            atime: t,
            mtime: t,
            ctime: t,
        })
    }

    fn statfs(&self) -> Option<StatFs> {
        Some(StatFs {
            total_bytes: TMPFS_CAP as u64,
            free_bytes: TMPFS_CAP.saturating_sub(self.used) as u64,
            block_size: 4096,
        })
    }
}
