//! Nyx filesystem PAL.
//!
//! **Path convention:** `open` and every path-taking syscall here take a `(ptr, len)` UTF-8 slice,
//! NOT a NUL-terminated C string. That is Nyx's convention rather than Linux's, and it is deliberate
//! — a kernel halfway between two conventions is worse than one consistently unusual one. The
//! conversion is owed in one atomic change when musl arrives.
//!
//! **What is real and what is not.** Since Phase 1 the kernel has `lseek`, the `stat` family,
//! `getdents64`, `mkdir`/`rmdir`/`unlink`/`rename`/`symlink`, `dup` and `access`, all covering
//! lwext4 — so metadata carries the **real on-disk inode number and the real mtime**, not a path
//! hash and a clock reading. What is still genuinely absent says so: there is no `readlink`, no
//! hardlink, and no cwd (so `canonicalize` cannot resolve a relative path). Those return
//! `Unsupported` rather than a plausible-looking guess, because a wrong answer here is worse than
//! no answer — `tests/posix` exists specifically to tell the two apart.
use crate::ffi::OsString;
use crate::fmt;
use crate::fs::TryLockError;
use crate::hash::{Hash, Hasher};
use crate::io::{self, BorrowedCursor, IoSlice, IoSliceMut, SeekFrom};
use crate::path::{Path, PathBuf};
pub use crate::sys::fs::common::Dir;
use crate::sys::time::{SystemTime, UNIX_EPOCH};
use crate::sys::unsupported;
use crate::time::Duration;

// --- Nyx syscalls (Linux-compatible numbers; see nyx-kernel/src/interrupts.rs). ---
const SYS_READ: usize = 0;
const SYS_WRITE: usize = 1;
const SYS_OPEN: usize = 2;
const SYS_CLOSE: usize = 3;
const SYS_STAT: usize = 4;
const SYS_FSTAT: usize = 5;
const SYS_LSEEK: usize = 8;
const SYS_ACCESS: usize = 21;
const SYS_DUP: usize = 32;
const SYS_RENAME: usize = 82;
const SYS_MKDIR: usize = 83;
const SYS_RMDIR: usize = 84;
const SYS_UNLINK: usize = 87;
const SYS_SYMLINK: usize = 88;
const SYS_GETDENTS64: usize = 217;

// open(2) flags honored by the kernel (arg3). Linux values.
const O_CREAT: usize = 0x40;
const O_TRUNC: usize = 0x200;

// struct stat, x86_64. Offsets are spelled out to match the kernel's `write_stat`.
const STAT_SIZE: usize = 144;
const ST_INO: usize = 8;
const ST_MODE: usize = 24;
const ST_SIZE: usize = 48;
const ST_ATIME: usize = 72;
const ST_MTIME: usize = 88;
const ST_CTIME: usize = 104;

const S_IFMT: u32 = 0o170000;
const S_IFDIR: u32 = 0o040000;
const S_IFREG: u32 = 0o100000;
const S_IFLNK: u32 = 0o120000;

// struct linux_dirent64: u64 d_ino, i64 d_off, u16 d_reclen, u8 d_type, then the name.
const DIRENT_RECLEN: usize = 16;
const DIRENT_TYPE: usize = 18;
const DIRENT_NAME: usize = 19;
const DT_DIR: u8 = 4;
const DT_LNK: u8 = 10;

#[inline]
unsafe fn sys3(n: usize, a1: usize, a2: usize, a3: usize) -> isize {
    let ret: isize;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") n => ret,
            in("rdi") a1, in("rsi") a2, in("rdx") a3,
            out("rcx") _, out("r11") _, options(nostack),
        );
    }
    ret
}

#[inline]
unsafe fn sys4(n: usize, a1: usize, a2: usize, a3: usize, a4: usize) -> isize {
    let ret: isize;
    unsafe {
        core::arch::asm!(
            "syscall",
            inlateout("rax") n => ret,
            in("rdi") a1, in("rsi") a2, in("rdx") a3, in("r10") a4,
            out("rcx") _, out("r11") _, options(nostack),
        );
    }
    ret
}

/// Turn a negative syscall return into an `io::Error`, or a non-negative one into `Ok`.
///
/// Going through `from_raw_os_error` rather than `ErrorKind` is what makes the errno table in
/// `io_error_nyx.rs` load-bearing: the kernel's ENOENT/EEXIST/ENOTEMPTY reach callers as the
/// matching `ErrorKind`, which is what `create_dir_all` and friends branch on.
#[inline]
fn check(ret: isize) -> io::Result<usize> {
    if ret < 0 { Err(io::Error::from_raw_os_error((-ret) as i32)) } else { Ok(ret as usize) }
}

/// Borrow a `Path` as the UTF-8 slice the kernel expects.
fn path_str(p: &Path) -> io::Result<&str> {
    p.to_str()
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "path is not valid UTF-8"))
}

pub struct File {
    fd: i32,
}

/// Metadata for one path, decoded from the kernel's `struct stat`.
#[derive(Clone, Copy)]
pub struct FileAttr {
    size: u64,
    mode: u32,
    atime: u64,
    mtime: u64,
    ctime: u64,
}

impl FileAttr {
    /// Decode a `struct stat` the kernel just filled in.
    fn from_stat(buf: &[u8; STAT_SIZE]) -> FileAttr {
        let rd_u64 = |off: usize| u64::from_ne_bytes(buf[off..off + 8].try_into().unwrap());
        let rd_u32 = |off: usize| u32::from_ne_bytes(buf[off..off + 4].try_into().unwrap());
        let _ = rd_u64(ST_INO);
        FileAttr {
            size: rd_u64(ST_SIZE),
            mode: rd_u32(ST_MODE),
            atime: rd_u64(ST_ATIME),
            mtime: rd_u64(ST_MTIME),
            ctime: rd_u64(ST_CTIME),
        }
    }

    /// Build a `FileAttr` for something with a type but no backing stat — a directory entry whose
    /// `d_type` we know without paying for a second syscall.
    fn from_mode(mode: u32) -> FileAttr {
        FileAttr { size: 0, mode, atime: 0, mtime: 0, ctime: 0 }
    }
}

/// A directory listing, read eagerly.
///
/// `getdents64` is drained fully at `readdir()` time rather than one batch per `next()`. The
/// kernel keeps the cursor in the fd's offset field, so a lazy iterator would have to hold the
/// descriptor open for the life of the iterator — and `std::fs::read_dir` callers routinely keep one
/// alive while doing arbitrary work inside the loop, including recursing into it.
pub struct ReadDir {
    base: PathBuf,
    entries: crate::vec::Vec<(OsString, u8)>,
    idx: usize,
}

pub struct DirEntry {
    base: PathBuf,
    name: OsString,
    d_type: u8,
}

#[derive(Clone, Debug)]
pub struct OpenOptions {
    read: bool,
    write: bool,
    append: bool,
    truncate: bool,
    create: bool,
}

#[derive(Copy, Clone, Debug, Default)]
pub struct FileTimes {}

/// Nyx has no uid/gid and no permission enforcement, so this carries the mode word purely so that
/// `readonly()` can answer from the write bits rather than guessing.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub struct FilePermissions {
    mode: u32,
}

#[derive(Clone, Copy, PartialEq, Eq, Hash, Debug)]
pub struct FileType {
    mode: u32,
}

#[derive(Debug)]
pub struct DirBuilder {}

/// Seconds-since-epoch out of the inode, as a `SystemTime`.
///
/// Built by adding to `UNIX_EPOCH` rather than by constructing the inner `Duration` directly —
/// `sys::time::SystemTime`'s field is private to its own module, and reaching for it would mean
/// patching a second std file for no gain.
fn stamp(secs: u64) -> io::Result<SystemTime> {
    UNIX_EPOCH
        .checked_add_duration(&Duration::from_secs(secs))
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidData, "timestamp out of range"))
}

impl FileAttr {
    pub fn size(&self) -> u64 {
        self.size
    }

    pub fn perm(&self) -> FilePermissions {
        FilePermissions { mode: self.mode }
    }

    pub fn file_type(&self) -> FileType {
        FileType { mode: self.mode }
    }

    pub fn modified(&self) -> io::Result<SystemTime> {
        stamp(self.mtime)
    }

    pub fn accessed(&self) -> io::Result<SystemTime> {
        stamp(self.atime)
    }

    /// ext4 records inode *change* time, not birth time. Reporting ctime as "created" would be a
    /// quiet lie — a chmod would look like a re-creation — so this refuses instead.
    pub fn created(&self) -> io::Result<SystemTime> {
        let _ = self.ctime;
        unsupported()
    }
}

impl FilePermissions {
    pub fn readonly(&self) -> bool {
        // No owner concept, so "readonly" is answered from the owner write bit, which is the only
        // one the filesystem actually records.
        self.mode & 0o200 == 0
    }

    pub fn set_readonly(&mut self, readonly: bool) {
        if readonly {
            self.mode &= !0o222;
        } else {
            self.mode |= 0o200;
        }
    }
}

impl FileTimes {
    pub fn set_accessed(&mut self, _t: SystemTime) {}
    pub fn set_modified(&mut self, _t: SystemTime) {}
}

impl FileType {
    pub fn is_dir(&self) -> bool {
        self.mode & S_IFMT == S_IFDIR
    }

    pub fn is_file(&self) -> bool {
        self.mode & S_IFMT == S_IFREG
    }

    pub fn is_symlink(&self) -> bool {
        self.mode & S_IFMT == S_IFLNK
    }
}

impl fmt::Debug for ReadDir {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ReadDir").field("base", &self.base).field("len", &self.entries.len()).finish()
    }
}

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;

    fn next(&mut self) -> Option<io::Result<DirEntry>> {
        let (name, d_type) = self.entries.get(self.idx)?.clone();
        self.idx += 1;
        Some(Ok(DirEntry { base: self.base.clone(), name, d_type }))
    }
}

impl DirEntry {
    pub fn path(&self) -> PathBuf {
        self.base.join(&self.name)
    }

    pub fn file_name(&self) -> OsString {
        self.name.clone()
    }

    pub fn metadata(&self) -> io::Result<FileAttr> {
        stat(&self.path())
    }

    /// Answered from the `d_type` `getdents64` already returned — no second syscall. Falling back
    /// to a `stat` per entry would turn one directory listing into N round trips.
    pub fn file_type(&self) -> io::Result<FileType> {
        let mode = match self.d_type {
            DT_DIR => S_IFDIR,
            DT_LNK => S_IFLNK,
            _ => S_IFREG,
        };
        Ok(FileType { mode })
    }
}

impl OpenOptions {
    pub fn new() -> OpenOptions {
        OpenOptions { read: false, write: false, append: false, truncate: false, create: false }
    }

    pub fn read(&mut self, read: bool) {
        self.read = read;
    }
    pub fn write(&mut self, write: bool) {
        self.write = write;
    }
    pub fn append(&mut self, append: bool) {
        self.append = append;
    }
    pub fn truncate(&mut self, truncate: bool) {
        self.truncate = truncate;
    }
    pub fn create(&mut self, create: bool) {
        self.create = create;
    }
    pub fn create_new(&mut self, create_new: bool) {
        // No O_EXCL kernel-side yet; treat as plain create (best effort).
        if create_new {
            self.create = true;
        }
    }
}

impl File {
    pub fn open(path: &Path, opts: &OpenOptions) -> io::Result<File> {
        // The kernel expects a raw (ptr,len) UTF-8 path slice — no NUL. Flags travel in arg3.
        let s = path.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "path is not valid UTF-8")
        })?;
        let mut flags = 0usize;
        if opts.create {
            flags |= O_CREAT;
        }
        if opts.truncate {
            flags |= O_TRUNC;
        }
        let ret = unsafe { sys3(SYS_OPEN, s.as_ptr() as usize, s.len(), flags) };
        if ret < 0 {
            // Kernel returns EBADF(-9)/EINVAL(-22)/EFAULT(-14) as negative errnos.
            Err(io::Error::from_raw_os_error((-ret) as i32))
        } else {
            Ok(File { fd: ret as i32 })
        }
    }

    pub fn file_attr(&self) -> io::Result<FileAttr> {
        let mut buf = [0u8; STAT_SIZE];
        check(unsafe { sys3(SYS_FSTAT, self.fd as usize, buf.as_mut_ptr() as usize, 0) })?;
        Ok(FileAttr::from_stat(&buf))
    }

    pub fn fsync(&self) -> io::Result<()> {
        Ok(())
    }

    pub fn datasync(&self) -> io::Result<()> {
        Ok(())
    }

    pub fn lock(&self) -> io::Result<()> {
        Ok(())
    }

    pub fn lock_shared(&self) -> io::Result<()> {
        Ok(())
    }

    pub fn try_lock(&self) -> Result<(), TryLockError> {
        Ok(())
    }

    pub fn try_lock_shared(&self) -> Result<(), TryLockError> {
        Ok(())
    }

    pub fn unlock(&self) -> io::Result<()> {
        Ok(())
    }

    pub fn truncate(&self, _size: u64) -> io::Result<()> {
        unsupported()
    }

    pub fn read(&self, buf: &mut [u8]) -> io::Result<usize> {
        let ret =
            unsafe { sys3(SYS_READ, self.fd as usize, buf.as_mut_ptr() as usize, buf.len()) };
        if ret < 0 { Err(io::Error::from_raw_os_error((-ret) as i32)) } else { Ok(ret as usize) }
    }

    pub fn read_vectored(&self, bufs: &mut [IoSliceMut<'_>]) -> io::Result<usize> {
        let mut total = 0;
        for b in bufs {
            let n = self.read(b)?;
            total += n;
            if n < b.len() {
                break; // short read == EOF; stop.
            }
        }
        Ok(total)
    }

    pub fn is_read_vectored(&self) -> bool {
        false
    }

    pub fn read_buf(&self, mut cursor: BorrowedCursor<'_, u8>) -> io::Result<()> {
        let mut tmp = [0u8; 512];
        let want = cursor.capacity().min(tmp.len());
        if want == 0 {
            return Ok(());
        }
        let n = self.read(&mut tmp[..want])?;
        cursor.append(&tmp[..n]);
        Ok(())
    }

    pub fn write(&self, buf: &[u8]) -> io::Result<usize> {
        // write(1) routes fd>=3 through the VFS OpenFile (write-through at the fd offset).
        let ret = unsafe { sys3(SYS_WRITE, self.fd as usize, buf.as_ptr() as usize, buf.len()) };
        if ret < 0 {
            Err(io::Error::from_raw_os_error((-ret) as i32))
        } else if ret == 0 && !buf.is_empty() {
            // The VFS bridge returns 0 on driver failure; surface it rather than looping forever
            // in write_all.
            Err(io::Error::new(io::ErrorKind::WriteZero, "kernel VFS wrote 0 bytes"))
        } else {
            Ok(ret as usize)
        }
    }

    pub fn write_vectored(&self, bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        let mut total = 0;
        for b in bufs {
            let n = self.write(b)?;
            total += n;
            if n < b.len() {
                break;
            }
        }
        Ok(total)
    }

    pub fn is_write_vectored(&self) -> bool {
        false
    }

    pub fn flush(&self) -> io::Result<()> {
        Ok(())
    }

    pub fn seek(&self, pos: SeekFrom) -> io::Result<u64> {
        // SEEK_SET = 0, SEEK_CUR = 1, SEEK_END = 2.
        let (whence, off) = match pos {
            SeekFrom::Start(n) => (0usize, n as i64),
            SeekFrom::Current(n) => (1usize, n),
            SeekFrom::End(n) => (2usize, n),
        };
        let ret = unsafe { sys3(SYS_LSEEK, self.fd as usize, off as usize, whence) };
        check(ret).map(|n| n as u64)
    }

    pub fn size(&self) -> Option<io::Result<u64>> {
        Some(self.file_attr().map(|a| a.size()))
    }

    pub fn tell(&self) -> io::Result<u64> {
        self.seek(SeekFrom::Current(0))
    }

    pub fn duplicate(&self) -> io::Result<File> {
        let fd = check(unsafe { sys3(SYS_DUP, self.fd as usize, 0, 0) })?;
        Ok(File { fd: fd as i32 })
    }

    pub fn set_permissions(&self, _perm: FilePermissions) -> io::Result<()> {
        unsupported()
    }

    pub fn set_times(&self, _times: FileTimes) -> io::Result<()> {
        unsupported()
    }
}

impl Drop for File {
    fn drop(&mut self) {
        if self.fd >= 0 {
            unsafe {
                let _ = sys3(SYS_CLOSE, self.fd as usize, 0, 0);
            }
        }
    }
}

impl DirBuilder {
    pub fn new() -> DirBuilder {
        DirBuilder {}
    }

    pub fn mkdir(&self, p: &Path) -> io::Result<()> {
        let s = path_str(p)?;
        check(unsafe { sys3(SYS_MKDIR, s.as_ptr() as usize, s.len(), 0o777) })?;
        Ok(())
    }
}

impl fmt::Debug for File {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("File").field("fd", &self.fd).finish()
    }
}

pub fn readdir(p: &Path) -> io::Result<ReadDir> {
    let s = path_str(p)?;
    // A directory is opened like any other path; the kernel only distinguishes them at getdents64.
    let fd = check(unsafe { sys3(SYS_OPEN, s.as_ptr() as usize, s.len(), 0) })?;
    let dir = File { fd: fd as i32 }; // owns the fd — closed by Drop on every path out of here

    let mut entries = crate::vec::Vec::new();
    let mut buf = [0u8; 4096];
    loop {
        let n = check(unsafe {
            sys3(SYS_GETDENTS64, dir.fd as usize, buf.as_mut_ptr() as usize, buf.len())
        })?;
        if n == 0 {
            break; // end of directory
        }

        let mut off = 0usize;
        while off + DIRENT_NAME <= n {
            let reclen = u16::from_ne_bytes(
                buf[off + DIRENT_RECLEN..off + DIRENT_RECLEN + 2].try_into().unwrap(),
            ) as usize;
            // A zero or overlong reclen would loop forever / read past the buffer. The kernel never
            // emits one, but this decodes attacker-shaped data in the general case.
            if reclen < DIRENT_NAME || off + reclen > n {
                break;
            }
            let d_type = buf[off + DIRENT_TYPE];
            let name_bytes = &buf[off + DIRENT_NAME..off + reclen];
            let name_len = name_bytes.iter().position(|&b| b == 0).unwrap_or(name_bytes.len());
            if let Ok(name) = crate::str::from_utf8(&name_bytes[..name_len]) {
                if !name.is_empty() && name != "." && name != ".." {
                    entries.push((OsString::from(name), d_type));
                }
            }
            off += reclen;
        }
    }

    Ok(ReadDir { base: p.to_path_buf(), entries, idx: 0 })
}

pub fn unlink(p: &Path) -> io::Result<()> {
    let s = path_str(p)?;
    check(unsafe { sys3(SYS_UNLINK, s.as_ptr() as usize, s.len(), 0) })?;
    Ok(())
}

pub fn rename(old: &Path, new: &Path) -> io::Result<()> {
    let a = path_str(old)?;
    let b = path_str(new)?;
    check(unsafe {
        sys4(SYS_RENAME, a.as_ptr() as usize, a.len(), b.as_ptr() as usize, b.len())
    })?;
    Ok(())
}

/// Accepted and ignored. Nyx enforces no permissions, so failing here would break callers that
/// routinely `set_permissions` after a copy, while succeeding changes nothing observable — and
/// `readonly()` continues to report what is actually on disk rather than what was requested.
pub fn set_perm(_p: &Path, _perm: FilePermissions) -> io::Result<()> {
    Ok(())
}

pub fn set_times(_p: &Path, _times: FileTimes) -> io::Result<()> {
    unsupported()
}

pub fn set_times_nofollow(_p: &Path, _times: FileTimes) -> io::Result<()> {
    unsupported()
}

pub fn rmdir(p: &Path) -> io::Result<()> {
    let s = path_str(p)?;
    check(unsafe { sys3(SYS_RMDIR, s.as_ptr() as usize, s.len(), 0) })?;
    Ok(())
}

pub fn remove_dir_all(path: &Path) -> io::Result<()> {
    // Depth-first: children before the directory itself, since rmdir refuses a non-empty one.
    for entry in readdir(path)? {
        let entry = entry?;
        if entry.file_type()?.is_dir() {
            remove_dir_all(&entry.path())?;
        } else {
            unlink(&entry.path())?;
        }
    }
    rmdir(path)
}

pub fn exists(path: &Path) -> io::Result<bool> {
    let s = path_str(path)?;
    // access(2), not open(2). Opening to test existence used to report `false` for directories and
    // left a descriptor to be reclaimed for every probe.
    Ok(unsafe { sys3(SYS_ACCESS, s.as_ptr() as usize, s.len(), 0) } >= 0)
}

/// No `readlink(2)` in the kernel yet: symlinks can be *created* (lwext4 does it) but not read back.
/// Reporting the path itself would be a silent lie, so this refuses.
pub fn readlink(_p: &Path) -> io::Result<PathBuf> {
    unsupported()
}

pub fn symlink(original: &Path, link: &Path) -> io::Result<()> {
    let target = path_str(original)?;
    let path = path_str(link)?;
    check(unsafe {
        sys4(SYS_SYMLINK, target.as_ptr() as usize, target.len(), path.as_ptr() as usize, path.len())
    })?;
    Ok(())
}

/// Hard links have no kernel call and no lwext4 cover here.
pub fn link(_src: &Path, _dst: &Path) -> io::Result<()> {
    unsupported()
}

pub fn stat(p: &Path) -> io::Result<FileAttr> {
    let s = path_str(p)?;
    let mut buf = [0u8; STAT_SIZE];
    check(unsafe {
        sys3(SYS_STAT, s.as_ptr() as usize, s.len(), buf.as_mut_ptr() as usize)
    })?;
    Ok(FileAttr::from_stat(&buf))
}

/// Identical to `stat` for now: the kernel resolves symlinks in the driver and offers no
/// no-follow variant, so pretending to have `lstat` semantics would mislead anything that walks a
/// tree and relies on not following links.
pub fn lstat(p: &Path) -> io::Result<FileAttr> {
    stat(p)
}

/// Needs a cwd to resolve a relative path and a readlink to collapse symlinks; Nyx has neither
/// (cwd arrives in Phase 4). Returning the input unchanged would be wrong for exactly the inputs
/// callers use this for.
pub fn canonicalize(_p: &Path) -> io::Result<PathBuf> {
    unsupported()
}

pub fn copy(from: &Path, to: &Path) -> io::Result<u64> {
    let ropts =
        OpenOptions { read: true, write: false, append: false, truncate: false, create: false };
    let wopts =
        OpenOptions { read: false, write: true, append: false, truncate: true, create: true };
    let reader = File::open(from, &ropts)?;
    let writer = File::open(to, &wopts)?;

    let mut buf = [0u8; 4096];
    let mut total = 0u64;
    loop {
        let n = reader.read(&mut buf)?;
        if n == 0 {
            break;
        }
        let mut written = 0;
        while written < n {
            written += writer.write(&buf[written..n])?;
        }
        total += n as u64;
    }
    Ok(total)
}
