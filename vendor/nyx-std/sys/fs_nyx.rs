//! Nyx filesystem PAL — real `File::open`/`read` over the Nyx open(2)/read(0)/close(3) syscalls.
//!
//! The kernel VFS is read-mostly: `open` takes a `(ptr,len)` path slice (NOT a C string + flags
//! like Linux) and returns an fd >= 3; `read` advances a per-fd offset and returns 0 at EOF; there
//! is no `seek`/`stat`/`size` syscall yet. So metadata/seek/write are left unsupported for now —
//! enough for `std::fs::read`/`File::open(..).read(..)`, which is the B-alpha milestone surface.
//! Everything else is copied verbatim from `sys/fs/unsupported.rs`.
use crate::ffi::OsString;
use crate::fmt;
use crate::fs::TryLockError;
use crate::hash::{Hash, Hasher};
use crate::io::{self, BorrowedCursor, IoSlice, IoSliceMut, SeekFrom};
use crate::path::{Path, PathBuf};
pub use crate::sys::fs::common::Dir;
use crate::sys::time::SystemTime;
use crate::sys::unsupported;

// --- Nyx syscalls (Linux-compatible numbers; see nyx-kernel/src/interrupts.rs). ---
const SYS_READ: usize = 0;
const SYS_OPEN: usize = 2;
const SYS_CLOSE: usize = 3;

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

pub struct File {
    fd: i32,
}

pub struct FileAttr(!);

pub struct ReadDir(!);

pub struct DirEntry(!);

#[derive(Clone, Debug)]
pub struct OpenOptions {
    read: bool,
    write: bool,
}

#[derive(Copy, Clone, Debug, Default)]
pub struct FileTimes {}

pub struct FilePermissions(!);

pub struct FileType(!);

#[derive(Debug)]
pub struct DirBuilder {}

impl FileAttr {
    pub fn size(&self) -> u64 {
        self.0
    }

    pub fn perm(&self) -> FilePermissions {
        self.0
    }

    pub fn file_type(&self) -> FileType {
        self.0
    }

    pub fn modified(&self) -> io::Result<SystemTime> {
        self.0
    }

    pub fn accessed(&self) -> io::Result<SystemTime> {
        self.0
    }

    pub fn created(&self) -> io::Result<SystemTime> {
        self.0
    }
}

impl Clone for FileAttr {
    fn clone(&self) -> FileAttr {
        self.0
    }
}

impl FilePermissions {
    pub fn readonly(&self) -> bool {
        self.0
    }

    pub fn set_readonly(&mut self, _readonly: bool) {
        self.0
    }
}

impl Clone for FilePermissions {
    fn clone(&self) -> FilePermissions {
        self.0
    }
}

impl PartialEq for FilePermissions {
    fn eq(&self, _other: &FilePermissions) -> bool {
        self.0
    }
}

impl Eq for FilePermissions {}

impl fmt::Debug for FilePermissions {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0
    }
}

impl FileTimes {
    pub fn set_accessed(&mut self, _t: SystemTime) {}
    pub fn set_modified(&mut self, _t: SystemTime) {}
}

impl FileType {
    pub fn is_dir(&self) -> bool {
        self.0
    }

    pub fn is_file(&self) -> bool {
        self.0
    }

    pub fn is_symlink(&self) -> bool {
        self.0
    }
}

impl Clone for FileType {
    fn clone(&self) -> FileType {
        self.0
    }
}

impl Copy for FileType {}

impl PartialEq for FileType {
    fn eq(&self, _other: &FileType) -> bool {
        self.0
    }
}

impl Eq for FileType {}

impl Hash for FileType {
    fn hash<H: Hasher>(&self, _h: &mut H) {
        self.0
    }
}

impl fmt::Debug for FileType {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0
    }
}

impl fmt::Debug for ReadDir {
    fn fmt(&self, _f: &mut fmt::Formatter<'_>) -> fmt::Result {
        self.0
    }
}

impl Iterator for ReadDir {
    type Item = io::Result<DirEntry>;

    fn next(&mut self) -> Option<io::Result<DirEntry>> {
        self.0
    }
}

impl DirEntry {
    pub fn path(&self) -> PathBuf {
        self.0
    }

    pub fn file_name(&self) -> OsString {
        self.0
    }

    pub fn metadata(&self) -> io::Result<FileAttr> {
        self.0
    }

    pub fn file_type(&self) -> io::Result<FileType> {
        self.0
    }
}

impl OpenOptions {
    pub fn new() -> OpenOptions {
        OpenOptions { read: false, write: false }
    }

    pub fn read(&mut self, read: bool) {
        self.read = read;
    }
    pub fn write(&mut self, write: bool) {
        self.write = write;
    }
    pub fn append(&mut self, _append: bool) {}
    pub fn truncate(&mut self, _truncate: bool) {}
    pub fn create(&mut self, _create: bool) {}
    pub fn create_new(&mut self, _create_new: bool) {}
}

impl File {
    pub fn open(path: &Path, _opts: &OpenOptions) -> io::Result<File> {
        // The kernel expects a raw (ptr,len) UTF-8 path slice — no NUL, no flags.
        let s = path.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "path is not valid UTF-8")
        })?;
        let ret = unsafe { sys3(SYS_OPEN, s.as_ptr() as usize, s.len(), 0) };
        if ret < 0 {
            // Kernel returns EBADF(-9)/EINVAL(-22)/EFAULT(-14) as negative errnos.
            Err(io::Error::from_raw_os_error((-ret) as i32))
        } else {
            Ok(File { fd: ret as i32 })
        }
    }

    pub fn file_attr(&self) -> io::Result<FileAttr> {
        unsupported()
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

    pub fn write(&self, _buf: &[u8]) -> io::Result<usize> {
        unsupported()
    }

    pub fn write_vectored(&self, _bufs: &[IoSlice<'_>]) -> io::Result<usize> {
        unsupported()
    }

    pub fn is_write_vectored(&self) -> bool {
        false
    }

    pub fn flush(&self) -> io::Result<()> {
        Ok(())
    }

    pub fn seek(&self, _pos: SeekFrom) -> io::Result<u64> {
        unsupported()
    }

    pub fn size(&self) -> Option<io::Result<u64>> {
        None
    }

    pub fn tell(&self) -> io::Result<u64> {
        unsupported()
    }

    pub fn duplicate(&self) -> io::Result<File> {
        unsupported()
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

    pub fn mkdir(&self, _p: &Path) -> io::Result<()> {
        unsupported()
    }
}

impl fmt::Debug for File {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("File").field("fd", &self.fd).finish()
    }
}

pub fn readdir(_p: &Path) -> io::Result<ReadDir> {
    unsupported()
}

pub fn unlink(_p: &Path) -> io::Result<()> {
    unsupported()
}

pub fn rename(_old: &Path, _new: &Path) -> io::Result<()> {
    unsupported()
}

pub fn set_perm(_p: &Path, perm: FilePermissions) -> io::Result<()> {
    match perm.0 {}
}

pub fn set_times(_p: &Path, _times: FileTimes) -> io::Result<()> {
    unsupported()
}

pub fn set_times_nofollow(_p: &Path, _times: FileTimes) -> io::Result<()> {
    unsupported()
}

pub fn rmdir(_p: &Path) -> io::Result<()> {
    unsupported()
}

pub fn remove_dir_all(_path: &Path) -> io::Result<()> {
    unsupported()
}

pub fn exists(_path: &Path) -> io::Result<bool> {
    // Best-effort: try to open it. Cheap and correct enough for read-only workloads.
    let opts = OpenOptions::new();
    match File::open(_path, &opts) {
        Ok(_) => Ok(true),
        Err(_) => Ok(false),
    }
}

pub fn readlink(_p: &Path) -> io::Result<PathBuf> {
    unsupported()
}

pub fn symlink(_original: &Path, _link: &Path) -> io::Result<()> {
    unsupported()
}

pub fn link(_src: &Path, _dst: &Path) -> io::Result<()> {
    unsupported()
}

pub fn stat(_p: &Path) -> io::Result<FileAttr> {
    unsupported()
}

pub fn lstat(_p: &Path) -> io::Result<FileAttr> {
    unsupported()
}

pub fn canonicalize(_p: &Path) -> io::Result<PathBuf> {
    unsupported()
}

pub fn copy(_from: &Path, _to: &Path) -> io::Result<u64> {
    unsupported()
}
