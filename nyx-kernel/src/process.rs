use x86_64::{PhysAddr, VirtAddr};
use alloc::vec::Vec;
use alloc::collections::VecDeque;
use crate::scheduler::{FileDescriptor, TaskState};
use core::sync::atomic::{AtomicU64, Ordering};

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64_Ehdr {
    pub e_ident: [u8; 16],
    pub e_type: u16,
    pub e_machine: u16,
    pub e_version: u32,
    pub e_entry: u64,
    pub e_phoff: u64,
    pub e_shoff: u64,
    pub e_flags: u32,
    pub e_ehsize: u16,
    pub e_phentsize: u16,
    pub e_phnum: u16,
    pub e_shentsize: u16,
    pub e_shnum: u16,
    pub e_shstrndx: u16,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct Elf64_Phdr {
    pub p_type: u32,
    pub p_flags: u32,
    pub p_offset: u64,
    pub p_vaddr: u64,
    pub p_paddr: u64,
    pub p_filesz: u64,
    pub p_memsz: u64,
    pub p_align: u64,
}
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct IpcMessage {
    pub sender_pid: u64,
    pub msg_type: u64,
    pub data1: u64,
    pub data2: u64,
}

/// TLS initialization image described by a PT_TLS program header. The PAL's runtime startup
/// (B3) reads this via AT_PHDR to lay out the initial thread's TLS block; we surface the fields
/// directly too so the kernel can install the main thread's block if we later choose to.
#[derive(Debug, Clone, Copy, Default)]
pub struct TlsTemplate {
    pub vaddr: u64,   // where the TLS init image sits in the loaded image
    pub filesz: u64,  // bytes of real init data (rest of memsz is zero-fill .tbss)
    pub memsz: u64,   // total per-thread TLS size
    pub align: u64,
}

/// Everything the SysV process startup (auxv) needs, produced by loading an ELF.
#[derive(Debug, Clone, Copy)]
pub struct LoadedElf {
    pub entry: u64,
    pub phdr_vaddr: u64, // in-memory address of the program header table (AT_PHDR), 0 if not mapped
    pub phent: u64,      // e_phentsize (AT_PHENT)
    pub phnum: u64,      // e_phnum (AT_PHNUM)
    pub tls: Option<TlsTemplate>,
}

/// Back-compat thin wrapper: load and return just the entry point.
pub fn load_elf(file_data: &[u8]) -> Result<u64, &'static str> {
    load_elf_full(file_data).map(|l| l.entry)
}

/// Master switch for Phase 3 step 3 (per-segment ELF protection).
///
/// Kept as a named constant because this is the highest-blast-radius change in the W^X work: one
/// segment protected wrongly kills every app at `_start` with nothing but `[SEGFAULT]` on serial.
/// Flipping this to `false` restores "every segment RWX" in one line, which is a far faster way to
/// confirm or clear this change as the cause of a bad boot than bisecting a revert.
const ENFORCE_ELF_PROT: bool = true;

/// Maximum open descriptors per process.
///
/// Was a bare `32` repeated at ~19 sites in the syscall dispatcher. Named, because the number is a
/// contract between the table's length and every bounds check that guards an index into it: an
/// out-of-range `fd_table[i]` panics, and a panic in the dispatcher takes down the KERNEL, not the
/// calling process. 32 was also simply too few — a browser opens more than that in sockets alone.
pub const FD_MAX: usize = 256;

/// Load all PT_LOAD segments and collect the metadata the SysV auxv needs (PT_PHDR/PT_TLS).
/// Static binaries only — PT_INTERP (dynamic) is rejected.
pub fn load_elf_full(file_data: &[u8]) -> Result<LoadedElf, &'static str> {
    if file_data.len() < core::mem::size_of::<Elf64_Ehdr>() { return Err("File too small"); }
    let header = unsafe { &*(file_data.as_ptr() as *const Elf64_Ehdr) };
    if header.e_ident[0..4] != [0x7F, b'E', b'L', b'F'] { return Err("Invalid ELF Magic"); }

    let ph_offset = header.e_phoff as usize;
    let ph_size = header.e_phentsize as usize;

    let mut tls: Option<TlsTemplate> = None;
    let mut phdr_vaddr: u64 = 0;
    // (page vaddr, union of p_flags of every PT_LOAD segment touching it). See the ★ block after
    // the loop.
    let mut page_prot: alloc::vec::Vec<(u64, u32)> = alloc::vec::Vec::new();

    for i in 0..header.e_phnum {
        let offset = ph_offset + (i as usize * ph_size);
        let phdr = unsafe { &*(file_data.as_ptr().add(offset) as *const Elf64_Phdr) };

        match phdr.p_type {
            2 => { // PT_DYNAMIC — a static binary has none; presence implies dynamic linking.
                return Err("Dynamic binaries unsupported (static only)");
            }
            3 => { // PT_INTERP — requires an ELF interpreter (ld.so); we are static-only.
                return Err("PT_INTERP unsupported (static only)");
            }
            6 => { // PT_PHDR — tells us where the program headers are mapped in the image.
                phdr_vaddr = phdr.p_vaddr;
            }
            7 => { // PT_TLS — thread-local storage init image.
                tls = Some(TlsTemplate {
                    vaddr: phdr.p_vaddr,
                    filesz: phdr.p_filesz,
                    memsz: phdr.p_memsz,
                    align: phdr.p_align.max(1),
                });
            }
            1 => { // PT_LOAD Segment
                // PIE FIX: Allow modern GCC binaries to load at address 0x0.
                // We only block them from loading into the Top Half (Kernel Space).
                if phdr.p_vaddr >= 0x0000_7FFF_FFFF_FFFF {
                    return Err("Security Violation: Cannot load into Kernel Space");
                }

                let start_page = phdr.p_vaddr & !0xFFF;
                let end_page = (phdr.p_vaddr + phdr.p_memsz + 0xFFF) & !0xFFF;
                let num_pages = ((end_page - start_page) / 4096) as usize;

                // Mapped writable AND executable here regardless of p_flags; the real protection is
                // applied in one pass after the loop. See the ★ block below for why it cannot be
                // done here.
                crate::memory::allocate_user_pages_at(start_page, num_pages, true)?;

                // Accumulate the UNION of p_flags per page as we go.
                for p in 0..num_pages {
                    let page = start_page + (p as u64) * 4096;
                    match page_prot.iter_mut().find(|e| e.0 == page) {
                        Some(e) => e.1 |= phdr.p_flags,
                        None => page_prot.push((page, phdr.p_flags)),
                    }
                }

                unsafe {
                    let dest = phdr.p_vaddr as *mut u8;
                    let src = file_data.as_ptr().add(phdr.p_offset as usize);
                    core::ptr::copy_nonoverlapping(src, dest, phdr.p_filesz as usize);

                    if phdr.p_memsz > phdr.p_filesz {
                        let bss_start = dest.add(phdr.p_filesz as usize);
                        let bss_len = phdr.p_memsz - phdr.p_filesz;
                        core::ptr::write_bytes(bss_start, 0, bss_len as usize);
                    }
                }

                // If no PT_PHDR was present, derive the phdr in-memory address from whichever
                // PT_LOAD segment actually contains the program-header file range (the common case
                // is the first LOAD at file offset 0, so phdr_vaddr = p_vaddr + e_phoff).
                if phdr_vaddr == 0 {
                    let ph_start = header.e_phoff;
                    let ph_end = ph_start + (header.e_phnum as u64) * (header.e_phentsize as u64);
                    if ph_start >= phdr.p_offset && ph_end <= phdr.p_offset + phdr.p_filesz {
                        phdr_vaddr = phdr.p_vaddr + (ph_start - phdr.p_offset);
                    }
                }
            }
            _ => {}
        }
    }

    // ★ Phase 3 step 3: apply each segment's real p_flags — but only AFTER every segment is
    // loaded, and per PAGE rather than per segment. Both parts are load-bearing:
    //
    //  - This function writes the segment contents itself. Mapping .text read-only up front would
    //    make our own `copy_nonoverlapping` fault in KERNEL mode, and a kernel-mode page fault
    //    panics the whole machine rather than killing one process.
    //  - ELF segments legitimately share a page — .rodata ending where .data or .tdata begins is
    //    the ordinary case, already documented in memory.rs. Stamping one segment's flags onto a
    //    shared page would revoke write access its neighbour needs, so each page gets the union of
    //    every segment sitting on it. The union can only ever be more permissive, never less, so
    //    this trades a little W^X strictness for never breaking a valid binary.
    if ENFORCE_ELF_PROT {
        const PF_X: u32 = 1;
        const PF_W: u32 = 2;
        let mut wx_pages = 0usize;
        for (page, f) in page_prot.iter() {
            let (w, x) = (f & PF_W != 0, f & PF_X != 0);
            if w && x { wx_pages += 1; }
            unsafe { crate::memory::protect_user_range(*page, 1, w, x)? };
        }
        // A page that ends up both writable and executable defeats the point of the exercise. It is
        // legal and we allow it, but it is worth naming rather than leaving silent.
        if wx_pages > 0 {
            crate::serial_println!(
                "[ELF] {} page(s) are both writable and executable (segments sharing a page)",
                wx_pages as u32);
        }
    }

    Ok(LoadedElf {
        entry: header.e_entry,
        phdr_vaddr,
        phent: header.e_phentsize as u64,
        phnum: header.e_phnum as u64,
        tls,
    })
}

// SysV x86-64 auxv tags we populate.
const AT_NULL: u64 = 0;
const AT_PHDR: u64 = 3;
const AT_PHENT: u64 = 4;
const AT_PHNUM: u64 = 5;
const AT_PAGESZ: u64 = 6;
const AT_ENTRY: u64 = 9;
const AT_RANDOM: u64 = 25;
const AT_HWCAP: u64 = 16;
// NYX-private auxv tags carrying the PT_TLS geometry directly (see build_initial_stack note). Chosen
// in the 0x7000_0000 range to avoid any real glibc AT_* tag; the nyx std PAL reads these.
const AT_NYX_TLS_VADDR: u64 = 0x7000_0001;
const AT_NYX_TLS_FILESZ: u64 = 0x7000_0002;
const AT_NYX_TLS_MEMSZ: u64 = 0x7000_0003;
const AT_NYX_TLS_ALIGN: u64 = 0x7000_0004;

/// Build the SysV-ABI initial process stack in the CURRENTLY MAPPED user address space and return
/// the RSP the new program must start on. Layout at entry (low → high address):
///
///   [ argc ][ argv[0..argc] ][ NULL ][ envp[0..] ][ NULL ][ auxv pairs ][ AT_NULL ]  ... strings
///
/// RSP points at `argc` and is 16-byte aligned, exactly as a fresh Linux process expects. no_std
/// apps that ignore all of this still work: they simply treat RSP as their stack top and grow down.
///
/// `stack_top` is the highest usable stack address (already page-mapped). `argv0` is the program
/// path (becomes argv[0]); we keep envp empty for now.
pub unsafe fn build_initial_stack(stack_top: u64, argv0: &str, elf: &LoadedElf) -> u64 {
    // A small bump area growing DOWN from the top holds the string/aux data the pointers reference.
    let mut p = stack_top & !0xF;

    // 1. argv[0] string (NUL-terminated).
    let bytes = argv0.as_bytes();
    p -= (bytes.len() + 1) as u64;
    let argv0_ptr = p;
    core::ptr::copy_nonoverlapping(bytes.as_ptr(), argv0_ptr as *mut u8, bytes.len());
    *((argv0_ptr + bytes.len() as u64) as *mut u8) = 0;

    // 2. 16 random bytes for AT_RANDOM (std/libc stack-guard + HashMap seed source).
    p -= 16;
    let random_ptr = p & !0xF;
    p = random_ptr;
    {
        let tsc: u64;
        core::arch::asm!("rdtsc", out("rax") tsc, out("rdx") _, options(nomem, nostack));
        let mut s = tsc ^ 0x9E37_79B9_7F4A_7C15;
        for i in 0..16u64 {
            s ^= s >> 12; s ^= s << 25; s ^= s >> 27;
            *((random_ptr + i) as *mut u8) = (s.wrapping_mul(0x2545_F491_4F6C_DD1D) >> 33) as u8;
        }
    }

    // 3. The auxv pairs (tag,val), terminated by AT_NULL. We also surface the PT_TLS geometry via
    //    NYX-private tags (0x7000_00xx): our linker.ld emits phdrs OUTSIDE any PT_LOAD segment and
    //    no PT_PHDR, so AT_PHDR is 0 and a std PAL can't walk phdrs to find PT_TLS. The kernel
    //    already parsed PT_TLS into `elf.tls`, so we hand its fields directly. no_std apps ignore
    //    all auxv, so this is invisible to them.
    let tls = elf.tls.unwrap_or_default();
    let auxv: [(u64, u64); 12] = [
        (AT_PHDR, elf.phdr_vaddr),
        (AT_PHENT, elf.phent),
        (AT_PHNUM, elf.phnum),
        (AT_PAGESZ, 4096),
        (AT_ENTRY, elf.entry),
        (AT_RANDOM, random_ptr),
        (AT_HWCAP, 0),
        (AT_NYX_TLS_VADDR, tls.vaddr),
        (AT_NYX_TLS_FILESZ, tls.filesz),
        (AT_NYX_TLS_MEMSZ, tls.memsz),
        (AT_NYX_TLS_ALIGN, tls.align),
        (AT_NULL, 0),
    ];

    // 4. Compute the info-block size (argc + argv[0] + argv NULL + envp NULL + auxv) and align the
    //    final RSP. Nyx's app `_start` is a plain `extern "C" fn`, so rustc assumes the SysV
    //    callee-entry convention RSP % 16 == 8 (as if reached by a `call`). We MUST reproduce that
    //    (the old bare-stack path did, via `& !0xF) - 8`) or every existing no_std app's prologue
    //    would misalign its SSE spills. argc still sits exactly at RSP; std's naked `_start` reads
    //    argc/argv/auxv relative to RSP and re-aligns to 16 before calling into Rust.
    let words: u64 = 1               // argc
        + 1                          // argv[0]
        + 1                          // argv NULL terminator
        + 1                          // envp NULL terminator
        + (auxv.len() as u64) * 2;   // auxv pairs (incl. AT_NULL pair)
    let mut sp = p - words * 8;
    sp &= !0xF; // 16-align...
    sp -= 8;    // ...then bias to RSP % 16 == 8 to match the Nyx _start ABI

    let mut w = sp as *mut u64;
    *w = 1; w = w.add(1);            // argc = 1
    *w = argv0_ptr; w = w.add(1);   // argv[0]
    *w = 0; w = w.add(1);           // argv NULL
    *w = 0; w = w.add(1);           // envp NULL
    for (tag, val) in auxv.iter() {
        *w = *tag; w = w.add(1);
        *w = *val; w = w.add(1);
    }

    sp
}

pub unsafe fn enter_userspace(entry: u64, stack: u64) -> ! {
    core::arch::asm!(
        "cli",           
        "mov ds, ax",    
        "mov es, ax",
        "swapgs",        
        "push rax",      // SS (0x28 | 3 = 0x2B)
        "push rcx",      // RSP 
        "push r11",      // RFLAGS (0x202)
        "push rdx",      // CS (0x30 | 3 = 0x33)
        "push r8",       // RIP
        "iretq",
        in("rax") 0x2B_u64,   
        in("rcx") stack,
        in("r11") 0x202_u64,
        in("rdx") 0x33_u64,   
        in("r8") entry,      
        options(noreturn)
    );
}

static NEXT_PID: AtomicU64 = AtomicU64::new(1);

pub struct Process {
    pub pid: u64,
    pub parent_pid: Option<u64>,
    pub cr3: PhysAddr,               
    pub saved_rsp: u64,              
    pub kernel_stack_top: u64,       
    pub mmap_bump: u64,
    /// Open descriptors. A `Vec` rather than a 256-entry inline array on purpose: `Process` lives
    /// in `scheduler.tasks: Vec<Process>`, which reallocates, and an inline array would mean
    /// copying 4 KiB per process on every growth and every fork. Indexing syntax is unchanged, but
    /// it is always length `FD_MAX` — never grown or shrunk — so `fd_table[i]` is safe exactly when
    /// `i < FD_MAX`, and every caller still has to check.
    pub fd_table: alloc::vec::Vec<Option<FileDescriptor>>,
    pub state: TaskState,
    pub cpu_ticks: u64,      
    pub name: [u8; 16],      
    pub is_idle: bool, 
    // --- NEW: WAKE TIMER FOR SYS_SLEEP ---
    pub wake_tsc: u64,
    pub mailbox: VecDeque<IpcMessage>,
    // --- std port (B1) ---
    // The futex user-address this task is currently blocked on (0 = none). Matched together with
    // `cr3` so two processes with distinct address spaces don't cross-wake on the same numeric
    // address; threads of one process share `cr3`, so intra-process futexes match correctly.
    pub futex_addr: u64,
    // Exit status captured at SYS_EXIT so a parent's wait4 can retrieve it before the Zombie is reaped.
    pub exit_code: i64,
    // B-β.2a: per-task FS base, saved/restored on every context switch so threads that set their own
    // FS (native TLS via arch_prctl) keep distinct thread-locals across preemption. 0 = never set
    // (harmless for no_std apps that never touch FS).
    pub saved_fs_base: u64,
}

impl Process {
    pub fn new() -> Result<Self, &'static str> {
        let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);
        
        let kernel_stack = crate::memory::allocate_kernel_stack(4);
        let pml4_frame = crate::memory::allocate_frame().ok_or("OOM: CR3 allocation failed")?;
        crate::memory::clone_kernel_page_table(pml4_frame.start_address());

        Ok(Process {
            pid,
            parent_pid: None,
            cr3: pml4_frame.start_address(),
            saved_rsp: kernel_stack, 
            kernel_stack_top: kernel_stack,
            mmap_bump: 0x4000_0000_0000, 
            fd_table: alloc::vec![None; FD_MAX],
            state: TaskState::Ready,
            cpu_ticks: 0,
            name: [0; 16],
            is_idle: false,
            wake_tsc: 0,
            mailbox: VecDeque::new(), // Default to empty mailbox
            futex_addr: 0,
            exit_code: 0,
            saved_fs_base: 0,
        })
    }

    pub fn new_thread(parent_cr3: PhysAddr) -> Result<Self, &'static str> {
        let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);
        let kernel_stack = crate::memory::allocate_kernel_stack(4);

        Ok(Process {
            pid,
            parent_pid: None,
            cr3: parent_cr3,
            saved_rsp: kernel_stack,
            kernel_stack_top: kernel_stack,
            mmap_bump: 0x4000_0000_0000,
            fd_table: alloc::vec![None; FD_MAX],
            state: TaskState::Ready,
            cpu_ticks: 0,
            name: [0; 16],
            is_idle: false,
            wake_tsc: 0,
            mailbox: VecDeque::new(), // Default to empty mailbox
            futex_addr: 0,
            exit_code: 0,
            saved_fs_base: 0,
        })
    }

    /// Idle task for an AP core (B-β.2c). Identical to `new()` EXCEPT the PID comes from a reserved
    /// high range: `init_aps` runs BEFORE the boot daemons are created, and consuming ordinary PIDs
    /// there would shift the well-known boot numbering (thermal=1, idle=2, init=3, compositor=4 —
    /// apps hardcode COMPOSITOR_PID=4 in libs/gui/app.rs for their window IPC). One AP idle task per
    /// core taking PIDs 1..N broke every app's window request; the 0xFFFF_0000+ range can never
    /// collide with real processes.
    pub fn new_idle_ap() -> Result<Self, &'static str> {
        static NEXT_IDLE_PID: AtomicU64 = AtomicU64::new(0xFFFF_0000);
        let pid = NEXT_IDLE_PID.fetch_add(1, Ordering::Relaxed);

        let kernel_stack = crate::memory::allocate_kernel_stack(4);
        let pml4_frame = crate::memory::allocate_frame().ok_or("OOM: CR3 allocation failed")?;
        crate::memory::clone_kernel_page_table(pml4_frame.start_address());

        Ok(Process {
            pid,
            parent_pid: None,
            cr3: pml4_frame.start_address(),
            saved_rsp: kernel_stack,
            kernel_stack_top: kernel_stack,
            mmap_bump: 0x4000_0000_0000,
            fd_table: alloc::vec![None; FD_MAX],
            state: TaskState::Ready,
            cpu_ticks: 0,
            name: [0; 16],
            is_idle: true,
            wake_tsc: 0,
            mailbox: VecDeque::new(),
            futex_addr: 0,
            exit_code: 0,
            saved_fs_base: 0,
        })
    }
}

// ==========================================
// THE RING-0 IDLE TASK (PID 0 / C-STATE ENABLER)
// ==========================================
pub extern "C" fn nyx_idle_task() {
    loop {
        // This allows smoltcp to send the DHCP Discover and handle incoming ARP/TCP packets.
        crate::drivers::net::poll_network();
        // Ensure interrupts are ALWAYS enabled before halting, 
        // preventing the CPU from becoming permanently bricked.
        unsafe { x86_64::instructions::interrupts::enable_and_hlt(); }
    }
}