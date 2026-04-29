use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use crate::gdt;
use lazy_static::lazy_static;
use pic8259::ChainedPics;
use spin::Mutex;
use crate::fs;
use crate::gui::{Painter, Rect, Color};
use alloc::format;
use x86_64::VirtAddr;
use smoltcp::wire::{IpAddress, IpEndpoint, Ipv4Address};
use crate::scheduler::{FileDescriptor, KernelSocket, SocketKind};
use alloc::sync::Arc;
use core::sync::atomic::{AtomicU64, Ordering};
use x86_64::registers::model_specific::GsBase;

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static PICS: Mutex<ChainedPics> = Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });
pub static KERNEL_CR3: AtomicU64 = AtomicU64::new(0);

const EBADF: i64 = -9;
const EAGAIN: i64 = -11;
const ENOMEM: i64 = -12;
const EFAULT: i64 = -14; 
const EINVAL: i64 = -22;
const EMFILE: i64 = -24;
const ENOSYS: i64 = -38; 

#[repr(C)]
pub struct SockAddrIn {
    pub sin_family: u16,
    pub sin_port: u16,
    pub sin_addr: [u8; 4],
    pub sin_zero: [u8; 8],
}

pub fn is_valid_user_ptr(ptr: *const u8, len: usize) -> bool {
    let start = ptr as u64;
    // PIE FIX: Allow reading from address 0x0 so the C program can find its strings!
    if start == 0 && len == 0 { return false; }
    if let Some(end) = start.checked_add(len as u64) {
        return end <= 0x0000_7FFF_FFFF_FFFF;
    }
    false
}

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
            idt[InterruptIndex::Keyboard.as_usize()].set_handler_addr(VirtAddr::new(keyboard_interrupt_stub as *const () as u64));
            idt[InterruptIndex::Mouse.as_usize()].set_handler_addr(VirtAddr::new(mouse_interrupt_stub as *const () as u64));
            idt[0x30].set_handler_addr(VirtAddr::new(ethernet_interrupt_stub as *const () as u64));
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

extern "x86-interrupt" fn double_fault_handler(stack_frame: InterruptStackFrame, _error_code: u64) -> ! {
    if (stack_frame.code_segment & 3) == 3 { unsafe { core::arch::asm!("swapgs", options(nostack)); } }
    panic!("EXCEPTION: DOUBLE FAULT");
}

extern "x86-interrupt" fn gpf_handler(stack_frame: InterruptStackFrame, error_code: u64) {
    if (stack_frame.code_segment & 3) == 3 { unsafe { core::arch::asm!("swapgs", options(nostack)); } }
    panic!("EXCEPTION: GPF Error: {} ({:#x})\nIP: {:#x}", error_code, error_code, stack_frame.instruction_pointer.as_u64());
}

extern "x86-interrupt" fn pf_handler(stack_frame: InterruptStackFrame, error_code: PageFaultErrorCode) {
    let was_user = (stack_frame.code_segment & 3) == 3;
    if was_user { unsafe { core::arch::asm!("swapgs", options(nostack)); } }

    let cr2 = x86_64::registers::control::Cr2::read();
    let cr3 = x86_64::registers::control::Cr3::read();
    
    if error_code.contains(PageFaultErrorCode::USER_MODE) {
        crate::serial_println!("\n[SEGFAULT] User Process Terminated. Invalid Memory Access at: {:?}", cr2);

        if GsBase::read().as_u64() != 0 {
            let percpu = crate::percpu::current();
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx < percpu.scheduler.tasks.len() {
                percpu.scheduler.tasks[curr_idx].state = crate::scheduler::TaskState::Zombie; 
                crate::memory::clear_user_address_space(percpu.scheduler.tasks[curr_idx].cr3);
            }
        }
        unsafe { 
            x86_64::instructions::interrupts::enable();
            loop { core::arch::asm!("hlt") } 
        }
    } else {
        if !was_user && (stack_frame.code_segment & 3) == 3 { unsafe { core::arch::asm!("swapgs", options(nostack)); } }
        panic!("KERNEL PAGE FAULT\nAddr: {:?}\nError: {:?}\nIP: {:#x}\nCS: {:#x}\nCR3: {:?}", 
            cr2, error_code, stack_frame.instruction_pointer.as_u64(), stack_frame.code_segment, cr3);
    }
}

core::arch::global_asm!(r#"
.global timer_interrupt_stub
timer_interrupt_stub:
    test qword ptr [rsp + 8], 3
    jz 1f
    swapgs
1:
    push rax; push rbx; push rcx; push rdx; push rbp; push rsi; push rdi
    push r8; push r9; push r10; push r11; push r12; push r13; push r14; push r15
    
    mov rax, rsp
    and rsp, -16
    sub rsp, 512
    fxsave [rsp]   
    
    sub rsp, 8
    push rax
    mov rdi, rsp
    
    call timer_context_switch
    
    mov rsp, rax
    pop rbx
    add rsp, 8
    
    fxrstor [rsp]
    mov rsp, rbx

    pop r15; pop r14; pop r13; pop r12; pop r11; pop r10; pop r9; pop r8
    pop rdi; pop rsi; pop rbp; pop rdx; pop rcx; pop rbx; pop rax

    test qword ptr [rsp + 8], 3
    jz 2f
    swapgs
2:
    iretq

.global keyboard_interrupt_stub
keyboard_interrupt_stub:
    test qword ptr [rsp + 8], 3
    jz 1f
    swapgs
1:
    push rax; push rbx; push rcx; push rdx; push rbp; push rsi; push rdi
    push r8; push r9; push r10; push r11; push r12; push r13; push r14; push r15

    mov rax, rsp
    and rsp, -16
    
    sub rsp, 8
    push rax
    call keyboard_handler_impl
    pop rax
    
    mov rsp, rax
    pop r15; pop r14; pop r13; pop r12; pop r11; pop r10; pop r9; pop r8
    pop rdi; pop rsi; pop rbp; pop rdx; pop rcx; pop rbx; pop rax

    test qword ptr [rsp + 8], 3
    jz 2f
    swapgs
2:
    iretq

.global mouse_interrupt_stub
mouse_interrupt_stub:
    test qword ptr [rsp + 8], 3
    jz 1f
    swapgs
1:
    push rax; push rbx; push rcx; push rdx; push rbp; push rsi; push rdi
    push r8; push r9; push r10; push r11; push r12; push r13; push r14; push r15

    mov rax, rsp
    and rsp, -16
    
    sub rsp, 8
    push rax
    call mouse_handler_impl
    pop rax
    
    mov rsp, rax
    pop r15; pop r14; pop r13; pop r12; pop r11; pop r10; pop r9; pop r8
    pop rdi; pop rsi; pop rbp; pop rdx; pop rcx; pop rbx; pop rax

    test qword ptr [rsp + 8], 3
    jz 2f
    swapgs
2:
    iretq

.global ethernet_interrupt_stub
ethernet_interrupt_stub:
    test qword ptr [rsp + 8], 3
    jz 1f
    swapgs
1:
    push rax; push rbx; push rcx; push rdx; push rbp; push rsi; push rdi
    push r8; push r9; push r10; push r11; push r12; push r13; push r14; push r15

    mov rax, rsp
    and rsp, -16
    
    sub rsp, 8
    push rax
    call ethernet_handler_impl
    pop rax
    
    mov rsp, rax
    pop r15; pop r14; pop r13; pop r12; pop r11; pop r10; pop r9; pop r8
    pop rdi; pop rsi; pop rbp; pop rdx; pop rcx; pop rbx; pop rax

    test qword ptr [rsp + 8], 3
    jz 2f
    swapgs
2:
    iretq
"#);

extern "C" { 
    fn timer_interrupt_stub(); 
    fn keyboard_interrupt_stub();
    fn mouse_interrupt_stub();
    fn ethernet_interrupt_stub();
    fn syscall_handler_asm();
}

#[no_mangle]
pub extern "C" fn timer_context_switch(current_rsp: u64) -> u64 {
    crate::apic::end_of_interrupt();
    if x86_64::registers::model_specific::GsBase::read().as_u64() == 0 { return current_rsp; }
    crate::percpu::current().scheduler.schedule(current_rsp)
}

#[no_mangle]
pub extern "C" fn keyboard_handler_impl() {
    use x86_64::instructions::port::Port;
    let mut port = Port::new(0x60);
    let scancode: u8 = unsafe { port.read() };
    crate::shell::handle_key(scancode);
    crate::apic::end_of_interrupt();
}

#[no_mangle]
pub extern "C" fn mouse_handler_impl() {
    use x86_64::instructions::port::Port;
    let mut port = Port::new(0x60);
    let packet_byte: u8 = unsafe { port.read() };
    crate::mouse::handle_interrupt(packet_byte);
    crate::apic::end_of_interrupt();
}

#[no_mangle]
pub extern "C" fn ethernet_handler_impl() {
    if let Some(mut driver_guard) = crate::drivers::net::NET_DRIVER.try_lock() {
        if let Some(driver) = driver_guard.as_mut() { driver.ack_interrupt(); }
    }
    core::sync::atomic::fence(core::sync::atomic::Ordering::Release);
    crate::drivers::net::NETWORK_PENDING.store(true, core::sync::atomic::Ordering::Release);
    crate::apic::end_of_interrupt();
}

pub fn init_syscalls() {
    use x86_64::registers::model_specific::{Efer, EferFlags, Msr};
    use x86_64::registers::rflags::RFlags;

    let cr3 = x86_64::registers::control::Cr3::read().0.start_address().as_u64();
    KERNEL_CR3.store(cr3, Ordering::SeqCst);

    unsafe {
        let mut cr0: u64;
        core::arch::asm!("mov {}, cr0", out(reg) cr0);
        cr0 &= !(1 << 2); 
        cr0 |= (1 << 1) | (1 << 5); 
        core::arch::asm!("mov cr0, {}", in(reg) cr0);

        let mut cr4: u64;
        core::arch::asm!("mov {}, cr4", out(reg) cr4);
        cr4 |= (1 << 9) | (1 << 10); 
        core::arch::asm!("mov cr4, {}", in(reg) cr4);

        Efer::update(|flags| *flags |= EferFlags::SYSTEM_CALL_EXTENSIONS);
        let mut star_msr = Msr::new(0xC0000081);
        star_msr.write((0x20_u64 << 48) | (0x08_u64 << 32)); 
        let mut lstar_msr = Msr::new(0xC0000082);
        lstar_msr.write(syscall_handler_asm as *const () as u64);
        let mut fmask_msr = Msr::new(0xC0000084);
        fmask_msr.write(RFlags::INTERRUPT_FLAG.bits());
    }
}

// FULL TRAPFRAME: Preserves all 15 registers so the Child process doesn't get Amnesia!
#[repr(C)]
pub struct SyscallStackFrame {
    pub r15: u64, pub r14: u64, pub r13: u64, pub r12: u64,
    pub r11: u64, pub r10: u64, pub r9:  u64, pub r8:  u64,
    pub rdi: u64, pub rsi: u64, pub rbp: u64, pub rdx: u64,
    pub rcx: u64, pub rbx: u64, pub rax: u64, 
}

core::arch::global_asm!(r#"
.global syscall_handler_asm
syscall_handler_asm:
    swapgs
    mov gs:[8], rsp   
    mov rsp, gs:[0]   

    // THE FORK FIX: Push ALL registers identical to the hardware timer
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

    mov rax, rsp
    mov rdi, rsp
    
    and rsp, -16
    sub rsp, 512
    fxsave [rsp]
    
    sub rsp, 8
    push rax
    
    call syscall_dispatcher
    
    pop rax
    add rsp, 8
    
    fxrstor [rsp]
    mov rsp, rax

    // Restore ALL registers
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

    mov rsp, gs:[8]
    swapgs
    sysretq
"#);

#[no_mangle]
pub extern "C" fn syscall_dispatcher(frame: &mut SyscallStackFrame) {
    if !is_valid_user_ptr(frame.rcx as *const u8, 1) { frame.rcx = 0; }
    if KERNEL_CR3.load(Ordering::Relaxed) == 0 { frame.rax = ENOSYS as u64; return; }
    if GsBase::read().as_u64() == 0 { frame.rax = ENOSYS as u64; return; }
    
    let percpu = crate::percpu::current();

    let id = frame.rax;
    let arg1 = frame.rdi;
    let arg2 = frame.rsi;
    let arg3 = frame.rdx;
    let arg4 = frame.r10; 
    let arg5 = frame.r8;

    match id {
        0 => { frame.rax = sys_read_internal(arg1 as usize, arg2 as *mut u8, arg3 as usize) as u64; },
        1 => { frame.rax = sys_write_internal(arg1 as usize, arg2 as *const u8, arg3 as usize) as u64; },
        2 => { 
            let buf_ptr = arg1 as *const u8;
            let len = arg2 as usize;
            
            if !is_valid_user_ptr(buf_ptr, len) { frame.rax = EFAULT as u64; return; }
            
            let path_slice = unsafe { core::slice::from_raw_parts(buf_ptr, len) };
            if let Ok(path) = core::str::from_utf8(path_slice) {
                if let Some(vnode) = crate::vfs::VFS.open_path(path) {
                    let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                    if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = EBADF as u64; return; }
                    
                    let task = &mut percpu.scheduler.tasks[curr_idx];
                    let mut allocated_fd = -1isize;
                    for i in 3..32 {
                        if task.fd_table[i].is_none() {
                            task.fd_table[i] = Some(crate::scheduler::FileDescriptor::File(
                                alloc::sync::Arc::new(crate::vfs::OpenFile::new(vnode))
                            ));
                            allocated_fd = i as isize;
                            break;
                        }
                    }
                    frame.rax = allocated_fd as u64; 
                } else { frame.rax = EBADF as u64; } 
            } else { frame.rax = EINVAL as u64; }
        },
        3 => { 
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = EBADF as u64; return; }
            let task = &mut percpu.scheduler.tasks[curr_idx];
            if arg1 < 32 { task.fd_table[arg1 as usize] = None; }
            frame.rax = 0;
        },
        9 => { 
            let addr = arg1 as u64;    
            let size = arg2 as usize;  
            let fd = arg5 as isize;    
            let offset = frame.r9 as usize; 

            if size == 0 || size > 0x200_0000 { frame.rax = ENOMEM as u64; return; }

            let num_pages = (size + 0xFFF) / 0x1000;
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = EBADF as u64; return; }
            
            let task = &mut percpu.scheduler.tasks[curr_idx];

            if fd == -1 {
                let target_addr = if addr == 0 {
                    let next_addr = task.mmap_bump;
                    task.mmap_bump += (num_pages as u64) * 0x1000;
                    next_addr
                } else { addr };

                match crate::memory::allocate_user_pages_at(target_addr, num_pages) {
                    Ok(mapped_addr) => frame.rax = mapped_addr,
                    Err(_) => frame.rax = ENOMEM as u64, 
                }
            } else {
                if fd >= 0 && fd < 32 {
                    if let Some(crate::scheduler::FileDescriptor::File(open_file)) = &task.fd_table[fd as usize] {
                        match open_file.node.mmap(offset, size) {
                            Ok(phys_addr) => {
                                if let Ok(virt_addr) = crate::memory::map_user_mmio(phys_addr, size) {
                                    frame.rax = virt_addr;
                                } else { frame.rax = ENOMEM as u64; }
                            },
                            Err(e) => frame.rax = e as u64,
                        }
                    } else { frame.rax = EBADF as u64; } 
                } else { frame.rax = EBADF as u64; } 
            }
        },
        12 => { // SYS_BRK (Data Segment Break for libc)
            // Trick musl into falling back to mmap
            frame.rax = 0; 
        },
        16 => { // SYS_IOCTL (I/O Control)
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = EBADF as u64; return; }
            let task = &mut percpu.scheduler.tasks[curr_idx];
            
            if arg1 < 32 {
                if let Some(FileDescriptor::File(open_file)) = &task.fd_table[arg1 as usize] {
                    match open_file.node.ioctl(arg2 as usize, arg3 as usize) {
                        Ok(res) => frame.rax = res as u64,
                        Err(e) => frame.rax = e as u64,
                    }
                } else { 
                    // ENOTTY (-25). Tells libc it's writing to a pipe, forcing flush
                    frame.rax = -25isize as u64; 
                }
            } else { frame.rax = EBADF as u64; }
        },
        20 => { // SYS_WRITEV 
            let fd = arg1 as usize;
            let iov_ptr = arg2 as *const u64; 
            let iovcnt = arg3 as usize;
            
            // Each iovec struct is 16 bytes (8 bytes for base pointer, 8 bytes for length)
            if !is_valid_user_ptr(iov_ptr as *const u8, iovcnt * 16) { 
                frame.rax = EFAULT as u64; 
                return; 
            }
            
            let mut total_written = 0isize;
            
            for i in 0..iovcnt {
                unsafe {
                    // Read the base memory address and length of the current buffer chunk
                    let base = *iov_ptr.add(i * 2);
                    let len = *iov_ptr.add(i * 2 + 1) as usize;
                    
                    if len > 0 {
                        let written = sys_write_internal(fd, base as *const u8, len);
                        if written < 0 {
                            if total_written == 0 { total_written = written; }
                            break;
                        }
                        total_written += written;
                    }
                }
            }
            frame.rax = total_written as u64;
        },
        22 => { // SYS_PIPE
            let fd_array_ptr = arg1 as *mut i32;
            if !is_valid_user_ptr(fd_array_ptr as *const u8, 8) { frame.rax = EFAULT as u64; return; }
            
            let pipe = alloc::sync::Arc::new(spin::Mutex::new(alloc::collections::VecDeque::<u8>::new()));
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            let task = &mut percpu.scheduler.tasks[curr_idx];
            
            let mut read_fd = -1;
            let mut write_fd = -1;
            for i in 3..32 {
                if task.fd_table[i].is_none() {
                    if read_fd == -1 { read_fd = i as i32; }
                    else if write_fd == -1 { write_fd = i as i32; break; }
                }
            }
            if read_fd != -1 && write_fd != -1 {
                task.fd_table[read_fd as usize] = Some(crate::scheduler::FileDescriptor::PipeRead(pipe.clone()));
                task.fd_table[write_fd as usize] = Some(crate::scheduler::FileDescriptor::PipeWrite(pipe));
                unsafe {
                    *fd_array_ptr.add(0) = read_fd;
                    *fd_array_ptr.add(1) = write_fd;
                }
                frame.rax = 0;
            } else { frame.rax = EMFILE as u64; }
        },
        33 => { // SYS_DUP2
            let oldfd = arg1 as usize;
            let newfd = arg2 as usize;
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            let task = &mut percpu.scheduler.tasks[curr_idx];
            
            if oldfd < 32 && newfd < 32 {
                if let Some(fd_obj) = task.fd_table[oldfd].clone() {
                    task.fd_table[newfd] = Some(fd_obj);
                    frame.rax = newfd as u64;
                } else { frame.rax = EBADF as u64; }
            } else { frame.rax = EBADF as u64; }
        },
        41 => frame.rax = sys_socket(arg1, arg2, arg3) as u64,
        42 => frame.rax = sys_connect(arg1 as usize, arg2 as *const u8, arg3 as usize) as u64,
        44 => frame.rax = sys_write_internal(arg1 as usize, arg2 as *const u8, arg3 as usize) as u64, 
        45 => frame.rax = sys_read_internal(arg1 as usize, arg2 as *mut u8, arg3 as usize) as u64,
        57 => { // SYS_FORK
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = ENOSYS as u64; return; }
            
            let mut child = crate::process::Process::new().expect("Failed to create child process");
            
            {
                let parent = &percpu.scheduler.tasks[curr_idx];
                child.parent_pid = Some(parent.pid);
                crate::memory::clone_user_address_space(parent.cr3, child.cr3);

                for i in 0..32 {
                    if let Some(fd) = &parent.fd_table[i] {
                        child.fd_table[i] = Some(fd.clone());
                    }
                }
            }

            let stack_top = child.kernel_stack_top;
            let iretq_ptr = stack_top - 40;
            unsafe {
                let iret_slice = core::slice::from_raw_parts_mut(iretq_ptr as *mut u64, 5);
                iret_slice[0] = frame.rcx;         
                iret_slice[1] = 0x33;              
                iret_slice[2] = frame.r11 | 0x200; 
                iret_slice[3] = percpu.user_rsp;   
                iret_slice[4] = 0x2B;              
            }

            let regs_ptr = iretq_ptr - 120;
            unsafe {
                let regs = core::slice::from_raw_parts_mut(regs_ptr as *mut u64, 15);
                regs[0] = frame.r15;
                regs[1] = frame.r14;
                regs[2] = frame.r13;
                regs[3] = frame.r12;
                regs[4] = frame.r11; 
                regs[5] = frame.r10; 
                regs[6] = frame.r9;  
                regs[7] = frame.r8;  
                regs[8] = frame.rdi; 
                regs[9] = frame.rsi; 
                regs[10] = frame.rbp; 
                regs[11] = frame.rdx; 
                regs[12] = frame.rcx; 
                regs[13] = frame.rbx; 
                regs[14] = 0; // The child receives PID 0!
            }

            let fxsave_ptr = (regs_ptr - 512) & !0xF;
            unsafe {
                core::ptr::write_bytes(fxsave_ptr as *mut u8, 0, 512);
                *(fxsave_ptr as *mut u32).add(6) = 0x1F80;
            }

            let final_rsp = fxsave_ptr - 16;
            unsafe {
                let bottom = core::slice::from_raw_parts_mut(final_rsp as *mut u64, 2);
                bottom[0] = regs_ptr; 
                bottom[1] = 0;        
            }

            child.saved_rsp = final_rsp;
            frame.rax = child.pid;
            
            percpu.scheduler.tasks.push(child);
        },   
        59 => { // SYS_EXECVE
            let buf_ptr = arg1 as *const u8;
            let len = arg2 as usize;
            
            if !is_valid_user_ptr(buf_ptr, len) { frame.rax = EFAULT as u64; return; }
            
            let path_slice = unsafe { core::slice::from_raw_parts(buf_ptr, len) };
            if let Ok(raw_path) = core::str::from_utf8(path_slice) {
                
                let path = raw_path.trim_matches(char::from(0)).trim();
                let elf_data_opt = if path.contains("hello.elf") {
                    Some(alloc::vec::Vec::from(crate::HELLO_BIN))
                } else {
                    let mut fs = crate::fs::FS.lock(); 
                    fs.read_file(path)
                };
                
                if let Some(elf_data) = elf_data_opt {
                    let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                    let task = &mut percpu.scheduler.tasks[curr_idx];

                    crate::memory::clear_user_address_space(task.cr3);

                    match crate::process::load_elf(&elf_data) {
                        Ok(entry_point) => {
                            let user_stack_base = 0x7FFF_0000_0000;
                            crate::memory::allocate_user_pages_at(user_stack_base, 4).expect("Failed stack allocation");
                            let user_stack_top = user_stack_base + (4 * 4096); 
                            
                            task.mmap_bump = 0x4000_0000_0000;

                            frame.rcx = entry_point;
                            frame.r11 = 0x202; 
                            
                            // 🚨 THE MUSL POSIX STACK FIX 🚨
                            // We lower the stack by 32 bytes and push a 0-filled 
                            // environment so Musl's `_start` reads valid terminators!
                            let initial_rsp = user_stack_top - 32;
                            unsafe {
                                let stack_ptr = initial_rsp as *mut u64;
                                *stack_ptr.add(0) = 0; // argc = 0
                                *stack_ptr.add(1) = 0; // argv NULL terminator
                                *stack_ptr.add(2) = 0; // envp NULL terminator
                                *stack_ptr.add(3) = 0; // auxv AT_NULL terminator
                                
                                percpu.user_rsp = initial_rsp; 
                            } 
                            return; 
                        },
                        Err(e) => {
                            crate::serial_println!("\n[KERNEL] Execve Failed: {}", e);
                            frame.rax = EINVAL as u64;
                        }
                    }
                } else { frame.rax = EINVAL as u64; } 
            } else { frame.rax = EINVAL as u64; } 
        },
        60 => { // SYS_EXIT 
            let exit_code = arg1 as i64;
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            let task = &mut percpu.scheduler.tasks[curr_idx];
            
            crate::serial_println!("[PID {}] Exited (Code: {})", task.pid, exit_code); 
            task.state = crate::scheduler::TaskState::Zombie; 

            // FD WIPE FIX: Destroys PipeWrite and signals EOF to Terminal!
            for i in 0..32 { task.fd_table[i] = None; }
            crate::memory::clear_user_address_space(task.cr3);

            unsafe {
                x86_64::instructions::interrupts::enable();
                loop { core::arch::asm!("hlt") }
            }
        },
        158 => { // SYS_ARCH_PRCTL (TLS Support for musl libc)
            let code = arg1;
            let addr = arg2;
            if code == 0x1002 { // ARCH_SET_FS
                unsafe { x86_64::registers::model_specific::FsBase::write(x86_64::VirtAddr::new(addr)); }
                frame.rax = 0;
            } else {
                frame.rax = EINVAL as u64;
            }
        },
        218 => { // SYS_SET_TID_ADDRESS (Set Thread ID for musl)
            frame.rax = 1;
        },
        501 => { 
             unsafe {
                 if let Some(p) = &mut crate::SCREEN_PAINTER {
                     let screen_w = p.info.width;
                     let screen_h = p.info.height;
                     let start_x = core::cmp::min(arg1 as usize, screen_w);
                     let start_y = core::cmp::min(arg2 as usize, screen_h);
                     let max_w = screen_w.saturating_sub(start_x);
                     let max_h = screen_h.saturating_sub(start_y);
                     let w = core::cmp::min(arg3 as usize, max_w);
                     let h = core::cmp::min(arg4 as usize, max_h);
                     let rect = Rect { x: start_x, y: start_y, w, h };
                     let color = match arg5 as u8 {
                         0 => Color::BLACK, 1 => Color::BLUE, 2 => Color::GREEN, 3 => Color::CYAN,
                         4 => Color::RED, 5 => Color::BLUE, 14 => Color::YELLOW, _ => Color::WHITE,
                     };
                     p.draw_rect(rect, color);
                 }
             }
        },
        504 => { 
            let mut lo: u32;
            let mut hi: u32;
            unsafe { core::arch::asm!("rdtsc", out("eax") lo, out("edx") hi) };
            let tsc = ((hi as u64) << 32) | (lo as u64);
            frame.rax = tsc / 2_000_000; 
        },
        505 => { 
            let m = crate::mouse::MOUSE_STATE.lock();
            frame.rax = (m.x as u64) << 32 | (m.y as u64) << 16 | (if m.left_click {1} else {0}) << 1 | (if m.right_click {1} else {0});
        },
        506 => { if let Some(c) = crate::shell::pop_key() { frame.rax = c as u64; } else { frame.rax = 0; } },
        507 => { 
            unsafe {
                 if let Some(p) = &crate::SCREEN_PAINTER {
                     if is_valid_user_ptr(arg1 as *const u8, 8) && is_valid_user_ptr(arg2 as *const u8, 8) && is_valid_user_ptr(arg3 as *const u8, 8) {
                         *(arg1 as *mut u64) = p.info.width as u64;
                         *(arg2 as *mut u64) = p.info.height as u64;
                         *(arg3 as *mut u64) = if p.info.stride > 0 { p.info.stride } else { p.info.width } as u64;
                         frame.rax = 1; 
                     } else { frame.rax = EFAULT as u64; }
                 } else { frame.rax = 0; }
            }
        },
        508 => { 
            unsafe {
                 if let Some(p) = &mut crate::SCREEN_PAINTER {
                     let virt_start = p.buffer.as_ptr() as u64;
                     if let Some(phys) = crate::memory::virt_to_phys(virt_start) {
                         if let Ok(user_virt) = crate::memory::map_user_framebuffer(phys, (p.buffer.len() * 4) as u64) {
                             frame.rax = user_virt; 
                         } else { frame.rax = 0; }
                     } else { frame.rax = 0; }
                 } else { frame.rax = 0; }
            }
        },
        510 => { 
            let buf_ptr = arg1 as *const u8;
            let len = arg2 as usize;
            if !is_valid_user_ptr(buf_ptr, len) { frame.rax = EFAULT as u64; return; }
            let slice = unsafe { core::slice::from_raw_parts(buf_ptr, len) };
            if let Ok(path) = core::str::from_utf8(slice) {
                frame.rax = crate::fs::FS.lock().ls(path).len() as u64; 
            } else { frame.rax = 0; }
        },
        511 => { 
            let idx = arg1 as usize;
            let buf_ptr = arg2 as *mut u8;
            let path_ptr = arg3 as *const u8;
            let path_len = arg4 as usize;

            if !is_valid_user_ptr(path_ptr, path_len) || !is_valid_user_ptr(buf_ptr, 256) { frame.rax = EFAULT as u64; return; }

            let path_slice = unsafe { core::slice::from_raw_parts(path_ptr, path_len) };
            if let Ok(path) = core::str::from_utf8(path_slice) {
                let mut fs = crate::fs::FS.lock();
                let files = fs.ls(path);
                if idx < files.len() {
                    let name = &files[idx];
                    unsafe {
                        let len = name.len();
                        for (i, b) in name.bytes().enumerate() { *buf_ptr.add(i) = b; }
                        frame.rax = len as u64;
                    }
                } else { frame.rax = 0; }
            } else { frame.rax = 0; }
        },
        517 => {
            let buf_ptr = arg1 as *mut u8;
            let buf_len = arg2 as usize;
            if !is_valid_user_ptr(buf_ptr, buf_len) { frame.rax = EFAULT as u64; return; }

            let mcfg = unsafe { crate::acpi::ACPI_INFO.mcfg_addr.unwrap_or(0) };
            let madt = unsafe { crate::acpi::ACPI_INFO.madt_addr.unwrap_or(0) };
            let info = format!("Hardware Discovery Report:\nMCFG: {:#x}\nMADT: {:#x}", mcfg, madt);
            let bytes = info.as_bytes();
            let len = core::cmp::min(bytes.len(), buf_len);
            unsafe { for i in 0..len { *buf_ptr.add(i) = bytes[i]; } }
            frame.rax = len as u64;
        },
        518 => { 
            let buf_ptr = arg1 as *mut u8;
            let buf_len = arg2 as usize;
            if !is_valid_user_ptr(buf_ptr, buf_len) { frame.rax = EFAULT as u64; return; }

            unsafe {
                let log_len = core::cmp::min(crate::serial::BOOT_LOG_IDX, 8192);
                let copy_len = core::cmp::min(buf_len, log_len);
                for i in 0..copy_len { *buf_ptr.add(i) = crate::serial::BOOT_LOG[i]; }
                frame.rax = copy_len as u64;
            }
        },
        519 => { 
            let num_pages = arg1 as usize;
            if num_pages == 0 || num_pages > 8192 { frame.rax = 0; return; }
            
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = 0; return; }
            
            let task = &mut percpu.scheduler.tasks[curr_idx];
            let target_addr = task.mmap_bump;
            task.mmap_bump += (num_pages as u64) * 0x1000;

            match crate::memory::allocate_user_pages_at(target_addr, num_pages) {
                Ok(mapped_addr) => frame.rax = mapped_addr,
                Err(_) => frame.rax = 0, 
            }
        },
        520 => { 
            let buf_ptr = arg1 as *mut u8;
            if arg2 as usize >= 32 && is_valid_user_ptr(buf_ptr, 32) { 
                unsafe { for i in 0..32 { *buf_ptr.add(i) = crate::entity::seed::GENETIC_SEED[i]; } }
                frame.rax = 1; 
            } else { frame.rax = EFAULT as u64; }
        },
        521 => { 
            let buf_ptr = arg1 as *mut f32;
            if arg2 as usize >= 4 && is_valid_user_ptr(buf_ptr as *const u8, 16) {
                unsafe {
                    *buf_ptr.add(0) = crate::entity::state::ENTITY_STATE.get_energy();
                    *buf_ptr.add(1) = crate::entity::state::ENTITY_STATE.get_entropy();
                    *buf_ptr.add(2) = crate::entity::state::ENTITY_STATE.get_stability();
                    *buf_ptr.add(3) = crate::entity::state::ENTITY_STATE.get_curiosity();
                }
                frame.rax = 1; 
            } else { frame.rax = EFAULT as u64; }
        },
        522 => { frame.rax = crate::smp::ACTIVE_CORES.load(Ordering::SeqCst) as u64; },
        523 => { frame.rax = crate::scheduler::CONTEXT_SWITCHES.load(Ordering::Relaxed); },
        _ => { frame.rax = EINVAL as u64; }
    }
}

fn sys_read_internal(fd: usize, buf_ptr: *mut u8, len: usize) -> isize {
    if !is_valid_user_ptr(buf_ptr, len) { return EFAULT as isize; }
    if len == 0 || fd >= 32 { return EBADF as isize; }
    
    if KERNEL_CR3.load(Ordering::Relaxed) == 0 { return EBADF as isize; }
    let percpu = crate::percpu::current();
    let p_addr = percpu as *const _ as u64;
    if unsafe { core::ptr::read_volatile(&p_addr) } == 0 { return EBADF as isize; }
    
    let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
    if curr_idx >= percpu.scheduler.tasks.len() { return EBADF as isize; }
    
    let task = &mut percpu.scheduler.tasks[curr_idx];
    
    if let Some(fd_enum) = &task.fd_table[fd] {
        match fd_enum {
            FileDescriptor::File(open_file) => {
                let buf_slice = unsafe { core::slice::from_raw_parts_mut(buf_ptr, len) };
                return open_file.read(buf_slice) as isize;
            },
            FileDescriptor::Socket(sock_mtx) => {
                crate::drivers::net::poll_network();
                let sock = sock_mtx.lock();
                let mut sockets_lock = crate::drivers::net::GLOBAL_SOCKETS.lock();
                if let Some(sockets) = sockets_lock.as_mut() {
                    let handle = match sock.kind { SocketKind::Udp(h) => h };
                    let socket = sockets.get_mut::<smoltcp::socket::udp::Socket>(handle);
                    if let Ok((data, _meta)) = socket.recv() {
                        let copy_len = core::cmp::min(data.len(), len);
                        unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), buf_ptr, copy_len); }
                        return copy_len as isize;
                    }
                }
                return EAGAIN as isize; 
            },
            FileDescriptor::PipeRead(pipe_mtx) => {
                let mut pipe = pipe_mtx.lock();
                let mut bytes_read = 0;
                while bytes_read < len {
                    if let Some(b) = pipe.pop_front() {
                        unsafe { *buf_ptr.add(bytes_read) = b; }
                        bytes_read += 1;
                    } else { break; }
                }
                
                if bytes_read == 0 { 
                    if alloc::sync::Arc::strong_count(pipe_mtx) == 1 {
                        return 0; // EOF
                    } else {
                        return EAGAIN as isize; 
                    }
                }
                return bytes_read as isize;
            },
            FileDescriptor::PipeWrite(_) => return EBADF as isize,
        }
    }
    EBADF as isize 
}

fn sys_write_internal(fd: usize, buf_ptr: *const u8, len: usize) -> isize {
    if !is_valid_user_ptr(buf_ptr, len) { return EFAULT as isize; }
    if len == 0 || fd >= 32 { return EBADF as isize; }
    
    if KERNEL_CR3.load(Ordering::Relaxed) == 0 { return EBADF as isize; }
    let percpu = crate::percpu::current();
    let p_addr = percpu as *const _ as u64;
    if unsafe { core::ptr::read_volatile(&p_addr) } == 0 { return EBADF as isize; }
    
    let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
    if curr_idx >= percpu.scheduler.tasks.len() { return EBADF as isize; }
    let task = &mut percpu.scheduler.tasks[curr_idx];
    
    let buf_slice = unsafe { core::slice::from_raw_parts(buf_ptr, len) };

    let mut bytes_sent = EAGAIN as isize;
    let mut trigger_net_poll = false;

    if let Some(fd_enum) = &task.fd_table[fd] {
        match fd_enum {
            FileDescriptor::File(open_file) => return open_file.write(buf_slice) as isize,
            FileDescriptor::Socket(sock_mtx) => {
                let sock = sock_mtx.lock();
                let mut sockets_lock = crate::drivers::net::GLOBAL_SOCKETS.lock();
                if let Some(sockets) = sockets_lock.as_mut() {
                    let handle = match sock.kind { SocketKind::Udp(h) => h };
                    let socket = sockets.get_mut::<smoltcp::socket::udp::Socket>(handle);
                    if let Some(endpoint) = sock.remote {
                        if socket.send_slice(buf_slice, endpoint).is_ok() { 
                            bytes_sent = buf_slice.len() as isize;
                            trigger_net_poll = true;
                        }
                    }
                }
            },
            FileDescriptor::PipeWrite(pipe_mtx) => {
                let mut pipe = pipe_mtx.lock();
                for &b in buf_slice { pipe.push_back(b); }
                return len as isize;
            },
            FileDescriptor::PipeRead(_) => return EBADF as isize,
        }
    }

    if trigger_net_poll {
        crate::drivers::net::poll_network();
        return bytes_sent;
    }

    if bytes_sent != (EAGAIN as isize) {
        return bytes_sent;
    }

    if fd == 1 || fd == 2 {
        if let Ok(s) = core::str::from_utf8(buf_slice) {
            crate::serial_print!("{}", s); 
        }
        return len as isize;
    }

    EBADF as isize
}

pub extern "C" fn sys_socket(_domain: u64, _typ: u64, _protocol: u64) -> i64 {
    let mut sockets_lock = crate::drivers::net::GLOBAL_SOCKETS.lock();
    if sockets_lock.is_none() { *sockets_lock = Some(smoltcp::iface::SocketSet::new(alloc::vec![])); }

    if let Some(sockets) = sockets_lock.as_mut() {
        let rx_buffer = smoltcp::socket::udp::PacketBuffer::new(alloc::vec![smoltcp::socket::udp::PacketMetadata::EMPTY; 8], alloc::vec![0; 2048]);
        let tx_buffer = smoltcp::socket::udp::PacketBuffer::new(alloc::vec![smoltcp::socket::udp::PacketMetadata::EMPTY; 8], alloc::vec![0; 2048]);

        let mut socket = smoltcp::socket::udp::Socket::new(rx_buffer, tx_buffer);
        let local_port = 49152 + (crate::time::get_ticks() % 10000) as u16;
        let _ = socket.bind(local_port);

        let handle = sockets.add(socket);
        let ks = KernelSocket { kind: SocketKind::Udp(handle), local_port, remote: None };

        if KERNEL_CR3.load(Ordering::Relaxed) == 0 { return EBADF; }
        let percpu = crate::percpu::current();
        let p_addr = percpu as *const _ as u64;
        if unsafe { core::ptr::read_volatile(&p_addr) } == 0 { return EBADF; }
        
        let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
        if curr_idx >= percpu.scheduler.tasks.len() { return EBADF; }
        
        let task = &mut percpu.scheduler.tasks[curr_idx];
        for i in 3..32 {
            if task.fd_table[i].is_none() {
                task.fd_table[i] = Some(FileDescriptor::Socket(Arc::new(Mutex::new(ks))));
                return i as i64;
            }
        }
    }
    -24 // EMFILE
}

pub extern "C" fn sys_connect(fd: usize, addr_ptr: *const u8, addr_len: usize) -> i64 {
    if addr_len < 16 || !is_valid_user_ptr(addr_ptr, addr_len) { return EFAULT; }
    
    if KERNEL_CR3.load(Ordering::Relaxed) == 0 { return EBADF; }
    let percpu = crate::percpu::current();
    let p_addr = percpu as *const _ as u64;
    if unsafe { core::ptr::read_volatile(&p_addr) } == 0 { return EBADF; }
    
    let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
    if curr_idx >= percpu.scheduler.tasks.len() { return EBADF; }
    
    let sockaddr = unsafe { &*(addr_ptr as *const SockAddrIn) };
    if sockaddr.sin_family != 2 { return EINVAL; }

    let port = u16::from_be(sockaddr.sin_port);
    let ip = sockaddr.sin_addr;

    let task = &mut percpu.scheduler.tasks[curr_idx];
    if fd < 32 {
        if let Some(FileDescriptor::Socket(sock_mtx)) = &task.fd_table[fd] {
            let mut sock = sock_mtx.lock();
            let addr = IpAddress::Ipv4(Ipv4Address::new(ip[0], ip[1], ip[2], ip[3]));
            sock.remote = Some(IpEndpoint::new(addr, port));
            return 0; 
        }
    }
    EBADF
}