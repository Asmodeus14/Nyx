//! Nyx std::process PAL (B-γ.3) — Command spawn/wait over fork(57) + execve(59) + wait4(61).
//!
//! Scope (matches the kernel):
//! * `execve` takes only a `(ptr,len)` path — NO argv/envp transfer. A Command with extra args or
//!   env changes fails with Unsupported instead of silently dropping them.
//! * stdio is always inherited (fd 1/2 → serial); pipes fall to `sys/pipe/unsupported`.
//! * `wait` blocks in wait4(-1-style targeted) and decodes the WEXITSTATUS byte the kernel packs.
//! * `kill` is unsupported (no signals on Nyx).
//!
//! Everything not listed above is copied from `sys/process/unsupported.rs`.
use super::env::{CommandEnv, CommandEnvs, CommandResolvedEnvs};
pub use crate::ffi::OsString as EnvKey;
use crate::ffi::{OsStr, OsString};
use crate::num::NonZero;
use crate::path::Path;
use crate::process::StdioPipes;
use crate::sys::fs::File;
use crate::sys::unsupported;
use crate::{fmt, io};

const SYS_FORK: usize = 57;
const SYS_EXECVE: usize = 59;
const SYS_WAIT4: usize = 61;
const SYS_EXIT: usize = 60;
const SYS_GETPID: usize = 39;

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

////////////////////////////////////////////////////////////////////////////////
// Command
////////////////////////////////////////////////////////////////////////////////

pub struct Command {
    program: OsString,
    args: Vec<OsString>,
    env: CommandEnv,

    cwd: Option<OsString>,
    stdin: Option<Stdio>,
    stdout: Option<Stdio>,
    stderr: Option<Stdio>,
}

#[derive(Debug)]
pub enum Stdio {
    Inherit,
    Null,
    MakePipe,
    ParentStdout,
    ParentStderr,
    #[allow(dead_code)] // This variant exists only for the Debug impl
    InheritFile(File),
}

impl Command {
    pub fn new(program: &OsStr) -> Command {
        Command {
            program: program.to_owned(),
            args: vec![program.to_owned()],
            env: Default::default(),
            cwd: None,
            stdin: None,
            stdout: None,
            stderr: None,
        }
    }

    pub fn arg(&mut self, arg: &OsStr) {
        self.args.push(arg.to_owned());
    }

    pub fn env_mut(&mut self) -> &mut CommandEnv {
        &mut self.env
    }

    pub fn cwd(&mut self, dir: &OsStr) {
        self.cwd = Some(dir.to_owned());
    }

    pub fn stdin(&mut self, stdin: Stdio) {
        self.stdin = Some(stdin);
    }

    pub fn stdout(&mut self, stdout: Stdio) {
        self.stdout = Some(stdout);
    }

    pub fn stderr(&mut self, stderr: Stdio) {
        self.stderr = Some(stderr);
    }

    pub fn get_program(&self) -> &OsStr {
        &self.program
    }

    pub fn get_args(&self) -> CommandArgs<'_> {
        let mut iter = self.args.iter();
        iter.next();
        CommandArgs { iter }
    }

    pub fn get_envs(&self) -> CommandEnvs<'_> {
        self.env.iter()
    }

    pub fn get_env_clear(&self) -> bool {
        self.env.does_clear()
    }

    pub fn get_resolved_envs(&self) -> CommandResolvedEnvs {
        CommandResolvedEnvs::new(self.env.capture())
    }

    pub fn get_current_dir(&self) -> Option<&Path> {
        self.cwd.as_ref().map(|cs| Path::new(cs))
    }

    pub fn spawn(
        &mut self,
        _default: Stdio,
        _needs_stdin: bool,
    ) -> io::Result<(Process, StdioPipes)> {
        // Kernel execve carries ONLY the path — refuse configs we can't honor instead of lying.
        if self.args.len() > 1 || !self.env.is_unchanged() || self.cwd.is_some() {
            return Err(io::Error::new(
                io::ErrorKind::Unsupported,
                "nyx execve carries no argv/env/cwd yet (path-only spawn)",
            ));
        }
        let path = self.program.to_str().ok_or_else(|| {
            io::Error::new(io::ErrorKind::InvalidInput, "program path is not valid UTF-8")
        })?;
        // Keep a stable copy: after fork both processes share this stack data (CoW), and the child
        // calls execve with pointers into it before returning anywhere.
        let bytes = path.as_bytes();

        let pid = unsafe { sys3(SYS_FORK, 0, 0, 0) };
        if pid < 0 {
            return Err(io::Error::from_raw_os_error((-pid) as i32));
        }
        if pid == 0 {
            // Child: replace ourselves with the target program. On success this never returns.
            unsafe {
                sys3(SYS_EXECVE, bytes.as_ptr() as usize, bytes.len(), 0);
                // execve failed (bad path?) — exit with a recognizable code; the parent's wait
                // sees 127, the conventional "command not found".
                sys3(SYS_EXIT, 127, 0, 0);
            }
            unreachable!("exit(60) returned");
        }

        let pipes = StdioPipes { stdin: None, stdout: None, stderr: None };
        Ok((Process { pid: pid as u64, status: None }, pipes))
    }
}

pub fn output(cmd: &mut Command) -> io::Result<(ExitStatus, Vec<u8>, Vec<u8>)> {
    // No pipes on Nyx yet: run the child with inherited stdio and return empty capture buffers.
    let (mut proc_, _pipes) = cmd.spawn(Stdio::Inherit, false)?;
    let status = proc_.wait()?;
    Ok((status, Vec::new(), Vec::new()))
}

impl From<ChildPipe> for Stdio {
    fn from(pipe: ChildPipe) -> Stdio {
        pipe.diverge()
    }
}

impl From<io::Stdout> for Stdio {
    fn from(_: io::Stdout) -> Stdio {
        Stdio::ParentStdout
    }
}

impl From<io::Stderr> for Stdio {
    fn from(_: io::Stderr) -> Stdio {
        Stdio::ParentStderr
    }
}

impl From<File> for Stdio {
    fn from(file: File) -> Stdio {
        Stdio::InheritFile(file)
    }
}

impl fmt::Debug for Command {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{:?}", self.program)?;
        for arg in &self.args[1..] {
            write!(f, " {:?}", arg)?;
        }
        Ok(())
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug, Default)]
pub struct ExitStatus(i32);

impl ExitStatus {
    pub fn exit_ok(&self) -> Result<(), ExitStatusError> {
        match NonZero::new(self.0) {
            None => Ok(()),
            Some(code) => Err(ExitStatusError(code)),
        }
    }

    pub fn code(&self) -> Option<i32> {
        Some(self.0)
    }
}

impl fmt::Display for ExitStatus {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "exit code: {}", self.0)
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct ExitStatusError(NonZero<i32>);

impl Into<ExitStatus> for ExitStatusError {
    fn into(self) -> ExitStatus {
        ExitStatus(self.0.get())
    }
}

impl ExitStatusError {
    pub fn code(self) -> Option<NonZero<i32>> {
        Some(self.0)
    }
}

#[derive(PartialEq, Eq, Clone, Copy, Debug)]
pub struct ExitCode(u8);

impl ExitCode {
    pub const SUCCESS: ExitCode = ExitCode(0);
    pub const FAILURE: ExitCode = ExitCode(1);

    pub fn as_i32(&self) -> i32 {
        self.0 as i32
    }
}

impl From<u8> for ExitCode {
    fn from(code: u8) -> Self {
        Self(code)
    }
}

pub struct Process {
    pid: u64,
    status: Option<ExitStatus>,
}

impl Process {
    pub fn id(&self) -> u32 {
        self.pid as u32
    }

    pub fn kill(&mut self) -> io::Result<()> {
        unsupported() // no signals on Nyx
    }

    pub fn wait(&mut self) -> io::Result<ExitStatus> {
        if let Some(st) = self.status {
            return Ok(st);
        }
        let mut wstatus: i32 = 0;
        let r = unsafe {
            sys3(SYS_WAIT4, self.pid as usize, &mut wstatus as *mut i32 as usize, 0)
        };
        if r < 0 {
            return Err(io::Error::from_raw_os_error((-r) as i32));
        }
        // Kernel packs WEXITSTATUS: (code & 0xff) << 8.
        let st = ExitStatus((wstatus >> 8) & 0xff);
        self.status = Some(st);
        Ok(st)
    }

    pub fn try_wait(&mut self) -> io::Result<Option<ExitStatus>> {
        if let Some(st) = self.status {
            return Ok(Some(st));
        }
        const WNOHANG: usize = 1;
        let mut wstatus: i32 = 0;
        let r = unsafe {
            sys3(SYS_WAIT4, self.pid as usize, &mut wstatus as *mut i32 as usize, WNOHANG)
        };
        if r < 0 {
            return Err(io::Error::from_raw_os_error((-r) as i32));
        }
        if r == 0 {
            return Ok(None); // still running
        }
        let st = ExitStatus((wstatus >> 8) & 0xff);
        self.status = Some(st);
        Ok(Some(st))
    }
}

pub struct CommandArgs<'a> {
    iter: crate::slice::Iter<'a, OsString>,
}

impl<'a> Iterator for CommandArgs<'a> {
    type Item = &'a OsStr;
    fn next(&mut self) -> Option<&'a OsStr> {
        self.iter.next().map(|os| &**os)
    }
    fn size_hint(&self) -> (usize, Option<usize>) {
        self.iter.size_hint()
    }
}

impl<'a> ExactSizeIterator for CommandArgs<'a> {
    fn len(&self) -> usize {
        self.iter.len()
    }
    fn is_empty(&self) -> bool {
        self.iter.is_empty()
    }
}

impl<'a> fmt::Debug for CommandArgs<'a> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_list().entries(self.iter.clone()).finish()
    }
}

pub type ChildPipe = crate::sys::pipe::Pipe;

pub fn read_output(
    out: ChildPipe,
    _stdout: &mut Vec<u8>,
    _err: ChildPipe,
    _stderr: &mut Vec<u8>,
) -> io::Result<()> {
    match out.diverge() {}
}

pub fn getpid() -> u32 {
    unsafe { sys3(SYS_GETPID, 0, 0, 0) as u32 }
}
