use x86_64::VirtAddr;
use alloc::vec::Vec;

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

        if phdr.p_type == 1 {
            if phdr.p_vaddr < 0x4000_0000 { return Err("Security Violation"); }
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
        
        //  MATCHING THE 9-SLOT GDT
        in("rax") 0x2B_u64,   // 0x28 (User Data) | 3
        in("rcx") stack,
        in("r11") 0x202_u64,
        in("rdx") 0x33_u64,   // 0x30 (User Code 64) | 3
        in("r8") entry,      
        options(noreturn)
    );
}