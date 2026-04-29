use x86_64::{PhysAddr, VirtAddr};
use alloc::vec::Vec;
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

pub fn load_elf(file_data: &[u8]) -> Result<u64, &'static str> {
    if file_data.len() < core::mem::size_of::<Elf64_Ehdr>() { return Err("File too small"); }
    let header = unsafe { &*(file_data.as_ptr() as *const Elf64_Ehdr) };
    if header.e_ident[0..4] != [0x7F, b'E', b'L', b'F'] { return Err("Invalid ELF Magic"); }

    let ph_offset = header.e_phoff as usize;
    let ph_size = header.e_phentsize as usize;
    
    for i in 0..header.e_phnum {
        let offset = ph_offset + (i as usize * ph_size);
        let phdr = unsafe { &*(file_data.as_ptr().add(offset) as *const Elf64_Phdr) };

        if phdr.p_type == 1 { // PT_LOAD Segment
            // 🚨 PIE FIX: Allow modern GCC binaries to load at address 0x0.
            // We only block them from loading into the Top Half (Kernel Space).
            if phdr.p_vaddr >= 0x0000_7FFF_FFFF_FFFF { 
                return Err("Security Violation: Cannot load into Kernel Space"); 
            }
            
            let start_page = phdr.p_vaddr & !0xFFF; 
            let end_page = (phdr.p_vaddr + phdr.p_memsz + 0xFFF) & !0xFFF; 
            let num_pages = ((end_page - start_page) / 4096) as usize;

            crate::memory::allocate_user_pages_at(start_page, num_pages)?;

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
        }
    }
    Ok(header.e_entry)
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
    pub fd_table: [Option<FileDescriptor>; 32],
    pub state: TaskState,
}

impl Process {
    pub fn new() -> Result<Self, &'static str> {
        let pid = NEXT_PID.fetch_add(1, Ordering::Relaxed);
        let pml4_frame = crate::memory::allocate_frame().ok_or("OOM: CR3 allocation failed")?;
        
        crate::memory::clone_kernel_page_table(pml4_frame.start_address());

        // 🚨 TELEPORT FIX: Ensure the new Kernel Stack maps to the Child's Brain
        let kernel_stack = unsafe {
            let old_cr3 = x86_64::registers::control::Cr3::read().0.start_address().as_u64();
            let child_cr3 = pml4_frame.start_address().as_u64();
            
            core::arch::asm!("mov cr3, {}", in(reg) child_cr3);
            let stack = crate::memory::allocate_kernel_stack(4);
            core::arch::asm!("mov cr3, {}", in(reg) old_cr3);
            
            stack
        };

        Ok(Process {
            pid,
            parent_pid: None,
            cr3: pml4_frame.start_address(),
            saved_rsp: kernel_stack, 
            kernel_stack_top: kernel_stack,
            mmap_bump: 0x4000_0000_0000, 
            fd_table: core::array::from_fn(|_| None),
            state: TaskState::Ready,
        })
    }
}