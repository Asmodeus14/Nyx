use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use crate::gdt;
use lazy_static::lazy_static;
use pic8259::ChainedPics;
use spin::Mutex;
use crate::fs;
use crate::gui::{Painter, Rect, Color};
use x86_64::PrivilegeLevel;
use alloc::format;
use x86_64::VirtAddr;

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static PICS: Mutex<ChainedPics> = Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        unsafe {
            idt.double_fault.set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        idt.page_fault.set_handler_fn(pf_handler);
        idt.general_protection_fault.set_handler_fn(gpf_handler);
        
        unsafe {
            idt[0x40].set_handler_addr(VirtAddr::new(timer_interrupt_stub as *const () as u64));
        }
        
        idt[InterruptIndex::Keyboard.as_usize()].set_handler_fn(keyboard_interrupt_handler);
        idt[InterruptIndex::Mouse.as_usize()].set_handler_fn(mouse_interrupt_handler);
        idt[0x30].set_handler_fn(ethernet_interrupt_handler);
        
        unsafe {
            idt[0x80].set_handler_addr(VirtAddr::new(syscall_handler_asm as *const () as u64))
                .set_privilege_level(PrivilegeLevel::Ring3);
        }

        idt
    };
}

pub fn init_idt() { IDT.load(); }

#[derive(Debug, Clone, Copy)]
#[repr(u8)]
pub enum InterruptIndex {
    Timer = PIC_1_OFFSET,
    Keyboard = PIC_1_OFFSET + 1,
    Mouse = PIC_2_OFFSET + 4,
}

impl InterruptIndex {
    fn as_usize(self) -> usize { self as usize }
    fn as_u8(self) -> u8 { self as u8 }
}

extern "x86-interrupt" fn breakpoint_handler(_stack_frame: InterruptStackFrame) {}

extern "x86-interrupt" fn double_fault_handler(_stack_frame: InterruptStackFrame, _error_code: u64) -> ! {
    panic!("EXCEPTION: DOUBLE FAULT");
}

extern "x86-interrupt" fn gpf_handler(stack_frame: InterruptStackFrame, error_code: u64) {
    panic!("EXCEPTION: GPF Error: {} ({:#x})\nIP: {:#x}", error_code, error_code, stack_frame.instruction_pointer.as_u64());
}

extern "x86-interrupt" fn pf_handler(stack_frame: InterruptStackFrame, error_code: PageFaultErrorCode) {
    let cr2 = x86_64::registers::control::Cr2::read();
    panic!("EXCEPTION: PAGE FAULT\nAddr: {:?}\nError: {:?}\nIP: {:#x}", 
        cr2, error_code, stack_frame.instruction_pointer.as_u64());
}

// ─────────────────────────────────────────────────────────────────────────
// SMP TIMER STUB
// ─────────────────────────────────────────────────────────────────────────
core::arch::global_asm!(r#"
.intel_syntax noprefix
.global timer_interrupt_stub
timer_interrupt_stub:
    test qword ptr [rsp + 8], 3
    jz 1f
    swapgs
1:
    push rax
    push rbx
    push rcx
    push rdx
    push rbp
    push rsi
    push rdi
    push r8
    push r9
    push r10
    push r11
    push r12
    push r13
    push r14
    push r15

    mov rdi, rsp
    call timer_context_switch
    mov rsp, rax

    pop r15
    pop r14
    pop r13
    pop r12
    pop r11
    pop r10
    pop r9
    pop r8
    pop rdi
    pop rsi
    pop rbp
    pop rdx
    pop rcx
    pop rbx
    pop rax

    test qword ptr [rsp + 8], 3
    jz 2f
    swapgs
2:
    iretq
"#);

extern "C" { fn timer_interrupt_stub(); }

#[no_mangle]
pub extern "C" fn timer_context_switch(current_rsp: u64) -> u64 {
    crate::apic::end_of_interrupt();
    crate::percpu::current().scheduler.schedule(current_rsp)
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;
    let mut port = Port::new(0x60);
    let scancode: u8 = unsafe { port.read() };
    crate::shell::handle_key(scancode);
    crate::apic::end_of_interrupt();
}

extern "x86-interrupt" fn mouse_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;
    let mut port = Port::new(0x60);
    let packet_byte: u8 = unsafe { port.read() };
    crate::mouse::handle_interrupt(packet_byte);
    crate::apic::end_of_interrupt();
}

pub fn init_syscalls() {}

#[repr(C)]
pub struct SyscallStackFrame {
    pub r11: u64, pub r10: u64, pub r9: u64, pub r8: u64,
    pub rcx: u64, pub rdx: u64, pub rsi: u64, pub rdi: u64,
    pub rax: u64,
}

// ─────────────────────────────────────────────────────────────────────────
// SMP SYSCALL STUB
// ─────────────────────────────────────────────────────────────────────────
core::arch::global_asm!(r#"
.intel_syntax noprefix
.global syscall_handler_asm
syscall_handler_asm:
    swapgs

    push rax
    push rdi
    push rsi
    push rdx
    push rcx
    push r8
    push r9
    push r10
    push r11
    
    mov rdi, rsp
    call syscall_dispatcher
    
    pop r11
    pop r10
    pop r9
    pop r8
    pop rcx
    pop rdx
    pop rsi
    pop rdi
    pop rax
    
    swapgs
    iretq
"#);

extern "C" { fn syscall_handler_asm(); }

#[no_mangle]
pub extern "C" fn syscall_dispatcher(frame: &mut SyscallStackFrame) {
    let id = frame.rax;
    let arg1 = frame.rdi;
    let arg2 = frame.rsi;
    let arg3 = frame.rdx;
    let arg4 = frame.rcx;
    let arg5 = frame.r8;

    match id {
        0 => {}, 
        1 => { // sys_draw_rect
             if let Some(p) = unsafe { &mut crate::SCREEN_PAINTER } {
                 let screen_w = p.info.width;
                 let screen_h = p.info.height;
                 
                 // 🚨 THE FIX: Completely clamp the coordinates so userspace can never overflow the VGA buffer
                 let start_x = core::cmp::min(arg1 as usize, screen_w);
                 let start_y = core::cmp::min(arg2 as usize, screen_h);
                 let max_w = screen_w.saturating_sub(start_x);
                 let max_h = screen_h.saturating_sub(start_y);
                 let w = core::cmp::min(arg3 as usize, max_w);
                 let h = core::cmp::min(arg4 as usize, max_h);
                 
                 let rect = Rect { x: start_x, y: start_y, w, h };
                 let color_code = arg5 as u8;
                 let color = match color_code {
                     0 => Color::BLACK, 1 => Color::BLUE, 2 => Color::GREEN, 3 => Color::CYAN,
                     4 => Color::RED, 5 => Color::BLUE, 14 => Color::YELLOW, _ => Color::WHITE,
                 };
                 p.draw_rect(rect, color);
             }
        },
        4 => { 
            let mut lo: u32;
            let mut hi: u32;
            unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi) };
            let tsc = ((hi as u64) << 32) | (lo as u64);
            frame.rax = tsc / 2_000_000; 
        },
        5 => { 
            let m = crate::mouse::MOUSE_STATE.lock();
            frame.rax = (m.x as u64) << 32 | (m.y as u64) << 16 | (if m.left_click {1} else {0}) << 1 | (if m.right_click {1} else {0});
        },
        6 => { if let Some(c) = crate::shell::pop_key() { frame.rax = c as u64; } else { frame.rax = 0; } },
        7 => { 
             if let Some(p) = unsafe { &crate::SCREEN_PAINTER } {
                 if arg1 != 0 && arg2 != 0 && arg3 != 0 {
                     unsafe {
                         *(arg1 as *mut u64) = p.info.width as u64;
                         *(arg2 as *mut u64) = p.info.height as u64;
                         *(arg3 as *mut u64) = if p.info.stride > 0 { p.info.stride } else { p.info.width } as u64;
                     }
                     frame.rax = 1; 
                 }
             } else { frame.rax = 0; }
        },
        8 => { 
             if let Some(p) = unsafe { &mut crate::SCREEN_PAINTER } {
                 let virt_start = p.buffer.as_ptr() as u64;
                 if let Some(phys) = crate::memory::virt_to_phys(virt_start) {
                     if let Ok(user_virt) = crate::memory::map_user_framebuffer(phys, p.buffer.len() as u64) {
                         frame.rax = user_virt; 
                     } else { frame.rax = 0; }
                 } else { frame.rax = 0; }
             } else { frame.rax = 0; }
        },
        9 => { // sys_mmap
            let fd = arg1 as usize;
            let size = arg2 as usize;
            let offset = arg3 as usize;

            unsafe {
                let percpu = crate::percpu::current();
                let core_id = percpu.logical_id;
                let scheduler = &mut percpu.scheduler;
                
                // 🚨 THE FIX: Use the stable core_task_idx tracker to prevent out of bounds panics
                let curr_idx = scheduler.core_task_idx[core_id % 32];

                if curr_idx < scheduler.tasks.len() {
                    let task = &mut scheduler.tasks[curr_idx];
                    if fd < 32 {
                        if let Some(open_file) = &task.fd_table[fd] {
                            match open_file.node.mmap(offset, size) {
                                Ok(phys_addr) => {
                                    if let Ok(virt_addr) = crate::memory::map_user_mmio(phys_addr, size) {
                                        frame.rax = virt_addr;
                                    } else { frame.rax = (-1isize) as u64; }
                                },
                                Err(e) => frame.rax = e as u64,
                            }
                        } else { frame.rax = (-1isize) as u64; }
                    } else { frame.rax = (-1isize) as u64; }
                } else { frame.rax = (-1isize) as u64; }
            }
        },
        10 => { // sys_fs_count
            let ptr = arg1 as *const u8;
            let len = arg2 as usize;
            let slice = unsafe { core::slice::from_raw_parts(ptr, len) };
            if let Ok(path) = core::str::from_utf8(slice) {
                let mut fs = fs::FS.lock();
                frame.rax = fs.ls(path).len() as u64;
            } else { frame.rax = 0; }
        },
        11 => { // sys_fs_get_name
            let idx = arg1 as usize;
            let buf_ptr = arg2 as *mut u8;
            let path_ptr = arg3 as *const u8;
            let path_len = arg4 as usize;
            let path_slice = unsafe { core::slice::from_raw_parts(path_ptr, path_len) };
            
            if let Ok(path) = core::str::from_utf8(path_slice) {
                let mut fs = fs::FS.lock();
                let files = fs.ls(path);
                if idx < files.len() {
                    let name = &files[idx];
                    unsafe {
                        let len = name.len();
                        let src = name.as_bytes();
                        
                        // 🚨 THE FIX: Reverted the bad clamp. We copy the full file name!
                        for i in 0..len { 
                            *buf_ptr.add(i) = src[i]; 
                        }
                        
                        frame.rax = len as u64;
                    }
                } else { frame.rax = 0; }
            } else { frame.rax = 0; }
        },
        12 => { // sys_fs_read
             let name_slice = unsafe { core::slice::from_raw_parts(arg1 as *const u8, arg2 as usize) };
             if let Ok(name) = core::str::from_utf8(name_slice) {
                 let mut fs = fs::FS.lock();
                 if let Some(data) = fs.read_file(name) {
                     unsafe {
                         for (i, byte) in data.iter().enumerate() { *(arg3 as *mut u8).add(i) = *byte; }
                         frame.rax = data.len() as u64;
                     }
                 } else { frame.rax = 0; }
             }
        },
        13 => { // sys_fs_write
             let name_slice = unsafe { core::slice::from_raw_parts(arg1 as *const u8, arg2 as usize) };
             let data_slice = unsafe { core::slice::from_raw_parts(arg3 as *const u8, arg4 as usize) };
             
             if let Ok(name) = core::str::from_utf8(name_slice) {
                 let mut fs = crate::fs::FS.lock();
                 let success = fs.write_file(name, data_slice);
                 frame.rax = if success { 1 } else { 0 }; 
             } else {
                 frame.rax = 0;
             }
        },
        14 => { 
            use core::sync::atomic::Ordering;
            frame.rax = crate::scheduler::CONTEXT_SWITCHES.load(Ordering::Relaxed);
        },
        15 => { // sys_open_file
            let path_slice = unsafe { core::slice::from_raw_parts(arg1 as *const u8, arg2 as usize) };
            if let Ok(path) = core::str::from_utf8(path_slice) {
                if let Some(vnode) = crate::vfs::VFS.open_path(path) {
                    unsafe {
                        let percpu = crate::percpu::current();
                        let core_id = percpu.logical_id;
                        let scheduler = &mut percpu.scheduler;
                        let curr_idx = scheduler.core_task_idx[core_id % 32];
                        
                        if curr_idx < scheduler.tasks.len() {
                            let task = &mut scheduler.tasks[curr_idx];
                            let mut allocated_fd = -1;
                            for i in 3..32 {
                                if task.fd_table[i].is_none() {
                                    task.fd_table[i] = Some(alloc::sync::Arc::new(crate::vfs::OpenFile::new(vnode)));
                                    allocated_fd = i as isize;
                                    break;
                                }
                            }
                            frame.rax = allocated_fd as u64; 
                        } else { frame.rax = (-1isize) as u64; }
                    }
                } else { frame.rax = (-1isize) as u64; } 
            } else { frame.rax = (-1isize) as u64; }
        },
        16 => { // sys_ioctl
            let fd = arg1 as usize;
            let request = arg2 as usize;
            let ioctl_arg = arg3 as usize;
            
            unsafe {
                let percpu = crate::percpu::current();
                let core_id = percpu.logical_id;
                let scheduler = &mut percpu.scheduler;
                let curr_idx = scheduler.core_task_idx[core_id % 32];
                
                if curr_idx < scheduler.tasks.len() {
                    let task = &mut scheduler.tasks[curr_idx];
                    if fd < 32 {
                        if let Some(open_file) = &task.fd_table[fd] {
                            match open_file.node.ioctl(request, ioctl_arg) {
                                Ok(res) => frame.rax = res as u64,
                                Err(e) => frame.rax = e as u64,
                            }
                        } else { frame.rax = (-1isize) as u64; }
                    } else { frame.rax = (-1isize) as u64; }
                } else { frame.rax = (-1isize) as u64; }
            }
        },
        17 => {
            let buf_ptr = arg1 as *mut u8;
            let buf_len = arg2 as usize;
            let mcfg = unsafe { crate::acpi::ACPI_INFO.mcfg_addr.unwrap_or(0) };
            let madt = unsafe { crate::acpi::ACPI_INFO.madt_addr.unwrap_or(0) };
            
            let info = alloc::format!(
                "Hardware Discovery Report:\n--------------------------\nACPI MCFG (PCIe): {:#010x}\nACPI MADT (APIC): {:#010x}\n\nLocal APIC: Enabled (MSI Ready)\nGPU Init: Waiting for Mesa", 
                mcfg, madt
            );
            
            let bytes = info.as_bytes();
            let len = core::cmp::min(bytes.len(), buf_len);
            unsafe { for i in 0..len { *buf_ptr.add(i) = bytes[i]; } }
            frame.rax = len as u64;
        },
        18 => { // sys_boot_log
            let buf_ptr = arg1 as *mut u8;
            let buf_len = arg2 as usize;
            unsafe {
                // 🚨 THE FIX: Clamp the boot log so userspace can't read out-of-bounds array data
                let log_len = core::cmp::min(crate::serial::BOOT_LOG_IDX, 8192);
                let copy_len = core::cmp::min(buf_len, log_len);
                
                for i in 0..copy_len {
                    *buf_ptr.add(i) = crate::serial::BOOT_LOG[i];
                }
                frame.rax = copy_len as u64;
            }
        },
        19 => {
            let num_pages = arg1 as usize;
            match crate::memory::allocate_user_heap_pages(num_pages) {
                Ok(virt_addr) => frame.rax = virt_addr,
                Err(_) => frame.rax = 0, 
            }
        },
        20 => {
            let buf_ptr = arg1 as *mut u8;
            let buf_len = arg2 as usize;
            if buf_len >= 32 { 
                unsafe {
                    let seed = &crate::entity::seed::GENETIC_SEED;
                    for i in 0..32 { *buf_ptr.add(i) = seed[i]; }
                }
                frame.rax = 1; 
            } else { frame.rax = 0; }
        },
        21 => {
            let buf_ptr = arg1 as *mut f32;
            let buf_len = arg2 as usize;
            if buf_len >= 4 {
                unsafe {
                    *buf_ptr.add(0) = crate::entity::state::ENTITY_STATE.get_energy();
                    *buf_ptr.add(1) = crate::entity::state::ENTITY_STATE.get_entropy();
                    *buf_ptr.add(2) = crate::entity::state::ENTITY_STATE.get_stability();
                    *buf_ptr.add(3) = crate::entity::state::ENTITY_STATE.get_curiosity();
                }
                frame.rax = 1; 
            } else { frame.rax = 0; }
        },
        22 => {
            use core::sync::atomic::Ordering;
            frame.rax = crate::smp::ACTIVE_CORES.load(Ordering::SeqCst) as u64;
        },
        _ => {}
    }
}

extern "x86-interrupt" fn ethernet_interrupt_handler(_stack: InterruptStackFrame) {
    //  THE FIX: Bind the lock guard first, then access the Option inside it
    let mut driver_guard = crate::drivers::net::NET_DRIVER.lock();
    if let Some(driver) = driver_guard.as_mut() {
        driver.ack_interrupt();
    }
    
    core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
    crate::drivers::net::NETWORK_PENDING.store(true, core::sync::atomic::Ordering::Release);
    crate::apic::end_of_interrupt();
}