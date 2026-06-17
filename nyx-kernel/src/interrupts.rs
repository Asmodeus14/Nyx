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
use core::sync::atomic::{AtomicU64, AtomicU16, Ordering};
use x86_64::registers::model_specific::GsBase;

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static PICS: Mutex<ChainedPics> = Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });
pub static KERNEL_CR3: AtomicU64 = AtomicU64::new(0);

// Atomic counter prevents Ephemeral Port exhaustion!
static NEXT_LOCAL_PORT: AtomicU16 = AtomicU16::new(49152);

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

#[repr(C)]
#[derive(Clone, Copy)]
pub struct TaskInfo {
    pub pid: u64,
    pub cpu_ticks: u64,
    pub state: u8, // 0 = Ready, 1 = Running, 2 = Blocked
    pub name: [u8; 16],
}

#[repr(C)]
pub struct SystemInfo {
    pub current_temp: u8,
    pub active_cooling: u8, // 1 = On, 0 = Off
    pub cpu_fan_rpm: u32,  
    pub gpu_fan_rpm: u32,
    pub task_count: u64,
    pub tasks: [TaskInfo; 64],
}

pub fn is_valid_user_ptr(ptr: *const u8, len: usize) -> bool {
    let start = ptr as u64;
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
            idt[0x41].set_handler_addr(VirtAddr::new(yield_interrupt_stub as *const () as u64));
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
    
    let cr2 = x86_64::registers::control::Cr2::read().as_u64();
    let cr3 = x86_64::registers::control::Cr3::read().0.start_address().as_u64();
    
    // 🚨 COPY-ON-WRITE (CoW) TRAP HANDLER 🚨
    if error_code.contains(PageFaultErrorCode::PROTECTION_VIOLATION) && error_code.contains(PageFaultErrorCode::CAUSED_BY_WRITE) {
        unsafe {
            let offset = crate::memory::PHYS_MEM_OFFSET;
            let pml4 = (cr3 + offset) as *mut u64;
            
            let i4 = (cr2 >> 39) & 0x1FF;
            let i3 = (cr2 >> 30) & 0x1FF;
            let i2 = (cr2 >> 21) & 0x1FF;
            let i1 = (cr2 >> 12) & 0x1FF;

            let pml4_entry = *pml4.add(i4 as usize);
            if pml4_entry & 1 != 0 {
                let pml3 = ((pml4_entry & 0x000FFFFF_FFFFF000) + offset) as *mut u64;
                let pml3_entry = *pml3.add(i3 as usize);
                if pml3_entry & 1 != 0 {
                    let pml2 = ((pml3_entry & 0x000FFFFF_FFFFF000) + offset) as *mut u64;
                    let pml2_entry = *pml2.add(i2 as usize);
                    if pml2_entry & 1 != 0 && (pml2_entry & (1 << 7)) == 0 {
                        let pt = ((pml2_entry & 0x000FFFFF_FFFFF000) + offset) as *mut u64;
                        let pt_entry = *pt.add(i1 as usize);
                        
                        // Check if Bit 10 (CoW Flag) is set!
                        if pt_entry & 0x400 != 0 {
                            // 1. Allocate a fresh physical frame
                            if let Some(new_frame) = crate::memory::allocate_frame() {
                                let old_phys = pt_entry & 0x000FFFFF_FFFFF000;
                                let new_phys = new_frame.start_address().as_u64();
                                
                                // 2. Copy the 4KB data from the old frame to the new frame
                                core::ptr::copy_nonoverlapping(
                                    (old_phys + offset) as *const u8,
                                    (new_phys + offset) as *mut u8,
                                    4096
                                );
                                
                                // 3. Update the PTE: Point to new frame, clear CoW bit, set Writable bit
                                let mut new_entry = (pt_entry & !0x000FFFFF_FFFFF000) | new_phys;
                                new_entry &= !0x400; // Clear CoW
                                new_entry |= 1 << 1; // Make Writable
                                *pt.add(i1 as usize) = new_entry;
                                
                                // 4. Flush the specific TLB page and instantly resume the app!
                                core::arch::asm!("invlpg [{}]", in(reg) cr2);
                                
                                if was_user { core::arch::asm!("swapgs", options(nostack)); }
                                return; // 🚨 BYPASS THE CRASH AND RESUME!
                            }
                        }
                    }
                }
            }
        }
    }

    if error_code.contains(PageFaultErrorCode::USER_MODE) {
        crate::serial_println!("\n[SEGFAULT] User Process Terminated. Invalid Memory Access at: {:#x}", cr2);
        if GsBase::read().as_u64() != 0 {
            let percpu = crate::percpu::current();
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx < percpu.scheduler.tasks.len() {
                let task = &mut percpu.scheduler.tasks[curr_idx];
                
                for i in 0..32 { 
                    if let Some(crate::scheduler::FileDescriptor::Socket(sock_mtx)) = &task.fd_table[i] {
                        if alloc::sync::Arc::strong_count(sock_mtx) == 1 {
                            let sock = sock_mtx.lock();
                            if let Some(sockets) = crate::drivers::net::GLOBAL_SOCKETS.lock().as_mut() {
                                match sock.kind {
                                    crate::scheduler::SocketKind::Tcp(handle) => {
                                        let socket = sockets.get_mut::<smoltcp::socket::tcp::Socket>(handle);
                                        socket.abort();
                                        sockets.remove(handle);
                                    },
                                    crate::scheduler::SocketKind::Udp(handle) => { sockets.remove(handle); }
                                }
                            }
                        }
                    }
                    task.fd_table[i] = None; 
                }

                crate::memory::clear_user_address_space(task.cr3);
                task.state = crate::scheduler::TaskState::Zombie;
            }
        }
        
        crate::apic::end_of_interrupt();
        
        unsafe { 
            x86_64::instructions::interrupts::enable();
            loop { core::arch::asm!("hlt") } 
        }
    } else {
        if !was_user && (stack_frame.code_segment & 3) == 3 { unsafe { core::arch::asm!("swapgs", options(nostack)); } }
        panic!("KERNEL PAGE FAULT\nAddr: {:#x}\nError: {:?}\nIP: {:#x}\nCS: {:#x}\nCR3: {:#x}", 
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

.global yield_interrupt_stub
yield_interrupt_stub:
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
    call yield_context_switch
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
    sub rsp, 512
    fxsave [rsp]
    sub rsp, 8
    push rax
    mov rdi, rsp
    call keyboard_context_switch
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
    sub rsp, 512
    fxsave [rsp]
    sub rsp, 8
    push rax
    mov rdi, rsp
    call mouse_context_switch
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
    fn yield_interrupt_stub();
}

#[no_mangle]
pub extern "C" fn timer_context_switch(current_rsp: u64) -> u64 {
    crate::apic::end_of_interrupt();
    
    // Safety check: Don't schedule if percpu isn't loaded
    if x86_64::registers::model_specific::GsBase::read().as_u64() == 0 { 
        return current_rsp; 
    }

    // --- THE TRUE WALL CLOCK ---
    crate::time::UPTIME_MS.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
    // ---------------------------
    
    let percpu = crate::percpu::current();
    
    // Increment the tick counter BEFORE we schedule a new task
    let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
    if curr_idx < percpu.scheduler.tasks.len() {
        percpu.scheduler.tasks[curr_idx].cpu_ticks += 1;
    }
    // ------------------------------------

    let new_rsp = percpu.scheduler.schedule(current_rsp);
    
    // Grab the NEXT task that the scheduler just picked
    let next_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
    if next_idx < percpu.scheduler.tasks.len() {
        let task = &percpu.scheduler.tasks[next_idx];
        let task_stack = task.kernel_stack_top;
        
        unsafe {
            // THE CRITICAL FIX: Swap CR3 to the new task's Address Space!
            let current_cr3 = x86_64::registers::control::Cr3::read().0.start_address().as_u64();
            if current_cr3 != task.cr3.as_u64() {
                core::arch::asm!("mov cr3, {}", in(reg) task.cr3.as_u64());
            }

            // Update Syscall and Hardware Interrupt Stacks
            let percpu_base = percpu as *const _ as *mut u64;
            *percpu_base = task_stack;
            
            let tss_ptr = percpu.gdt_state.tss as *const _ as *mut x86_64::structures::tss::TaskStateSegment;
            (*tss_ptr).privilege_stack_table[0] = x86_64::VirtAddr::new(task_stack);
        }
    }
    
    new_rsp
}

#[no_mangle]
pub extern "C" fn yield_context_switch(current_rsp: u64) -> u64 {
    // 🚨 NO EOI IS SENT HERE. This prevents APIC corruption! 🚨
    if x86_64::registers::model_specific::GsBase::read().as_u64() == 0 { return current_rsp; }
    
    let percpu = crate::percpu::current();
    let new_rsp = percpu.scheduler.schedule(current_rsp);
    
    let next_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
    if next_idx < percpu.scheduler.tasks.len() {
        let task = &percpu.scheduler.tasks[next_idx];
        let task_stack = task.kernel_stack_top;
        unsafe {
            let current_cr3 = x86_64::registers::control::Cr3::read().0.start_address().as_u64();
            if current_cr3 != task.cr3.as_u64() { core::arch::asm!("mov cr3, {}", in(reg) task.cr3.as_u64()); }
            let percpu_base = percpu as *const _ as *mut u64;
            *percpu_base = task_stack;
            let tss_ptr = percpu.gdt_state.tss as *const _ as *mut x86_64::structures::tss::TaskStateSegment;
            (*tss_ptr).privilege_stack_table[0] = x86_64::VirtAddr::new(task_stack);
        }
    }
    new_rsp
}
#[no_mangle]
pub extern "C" fn keyboard_context_switch(current_rsp: u64) -> u64 {
    // 1. Let the driver read the keystroke (This naturally drains port 0x60!)
    keyboard_handler_impl(); 
    
    // 2. SAFE EOI (Fired exactly ONCE!)
    crate::apic::end_of_interrupt(); 
    
    // 3. Human Input Override
    if x86_64::registers::model_specific::GsBase::read().as_u64() != 0 {
        let percpu = crate::percpu::current();
        for task in percpu.scheduler.tasks.iter_mut() {
            if task.state == crate::scheduler::TaskState::Blocked && task.wake_tsc > 0 && task.wake_tsc != u64::MAX {
                task.state = crate::scheduler::TaskState::Ready;
                task.wake_tsc = 0;
            }
        }
    }
    yield_context_switch(current_rsp) 
}

#[no_mangle]
pub extern "C" fn mouse_context_switch(current_rsp: u64) -> u64 {
    // 1. Let the driver read the mouse movement (This naturally drains port 0x60!)
    mouse_handler_impl(); 
    
    // 2. SAFE EOI (Fired exactly ONCE!)
    crate::apic::end_of_interrupt(); 
    
    // 3. Human Input Override
    if x86_64::registers::model_specific::GsBase::read().as_u64() != 0 {
        let percpu = crate::percpu::current();
        for task in percpu.scheduler.tasks.iter_mut() {
            if task.state == crate::scheduler::TaskState::Blocked && task.wake_tsc > 0 && task.wake_tsc != u64::MAX {
                task.state = crate::scheduler::TaskState::Ready;
                task.wake_tsc = 0;
            }
        }
    }
    yield_context_switch(current_rsp) 
}

#[no_mangle]
pub extern "C" fn keyboard_handler_impl() {
    use x86_64::instructions::port::Port;
    let mut port = Port::new(0x60);
    let scancode: u8 = unsafe { port.read() };
    crate::shell::handle_key(scancode);
    // 🚨 EOI REMOVED FROM HERE!
}

#[no_mangle]
pub extern "C" fn mouse_handler_impl() {
    use x86_64::instructions::port::Port;
    let mut port = Port::new(0x60);
    let packet_byte: u8 = unsafe { port.read() };
    crate::mouse::handle_interrupt(packet_byte);
    // 🚨 EOI REMOVED FROM HERE!
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

#[repr(C)]
pub struct SyscallStackFrame {
    pub r15: u64, pub r14: u64, pub r13: u64, pub r12: u64,
    pub r11: u64, pub r10: u64, pub r9:  u64, pub r8:  u64,
    pub rdi: u64, pub rsi: u64, pub rbp: u64, pub rdx: u64,
    pub rcx: u64, pub rbx: u64, pub rax: u64, 
    pub user_rsp: u64, // <--- ADD THIS AT THE BOTTOM
}

core::arch::global_asm!(r#"
.global syscall_handler_asm
syscall_handler_asm:
    swapgs
    mov gs:[8], rsp           
    mov rsp, gs:[0]           
    
    push qword ptr gs:[8]    // <--- PUSH USER RSP INTO THE FRAME
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
    pop qword ptr gs:[8]    // <--- POP IT SAFELY BACK

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
        3 => { // SYS_CLOSE
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = EBADF as u64; return; }
            let task = &mut percpu.scheduler.tasks[curr_idx];

            if arg1 < 32 { 
                // Cleanly tear down TCP sockets to avoid Windows NAT exhaustion!
                if let Some(FileDescriptor::Socket(sock_mtx)) = &task.fd_table[arg1 as usize] {
                    let sock = sock_mtx.lock();
                    if let Some(sockets) = crate::drivers::net::GLOBAL_SOCKETS.lock().as_mut() {
                        match sock.kind {
                            SocketKind::Tcp(handle) => {
                                let socket = sockets.get_mut::<smoltcp::socket::tcp::Socket>(handle);
                                socket.abort(); // Send TCP RST
                                sockets.remove(handle); // Free the memory
                            },
                            SocketKind::Udp(handle) => {
                                sockets.remove(handle);
                            }
                        }
                    }
                }
                task.fd_table[arg1 as usize] = None; 
            }
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
                        match open_file.mmap(offset, size){
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
        
        10 => { frame.rax = 0; }, // SYS_MPROTECT
        12 => { frame.rax = 0; }, // SYS_BRK 
        13 => { frame.rax = 0; }, // SYS_RT_SIGACTION
        14 => { frame.rax = 0; }, // SYS_RT_SIGPROCMASK
        
        16 => { // SYS_IOCTL 
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = EBADF as u64; return; }
            let task = &mut percpu.scheduler.tasks[curr_idx];
            
            if arg1 < 32 {
                if let Some(FileDescriptor::File(open_file)) = &task.fd_table[arg1 as usize] {
                    match open_file.ioctl(arg2 as usize, arg3 as usize) {
                        Ok(res) => frame.rax = res as u64,
                        Err(e) => frame.rax = e as u64,
                    }
                } else { 
                    frame.rax = -25isize as u64; // ENOTTY
                }
            } else { frame.rax = EBADF as u64; }
        },
        
        20 => { // SYS_WRITEV 
            let fd = arg1 as usize;
            let iov_ptr = arg2 as *const u64; 
            let iovcnt = arg3 as usize;
            
            if !is_valid_user_ptr(iov_ptr as *const u8, iovcnt * 16) { 
                frame.rax = EFAULT as u64; 
                return; 
            }
            
            let mut total_written = 0isize;
            for i in 0..iovcnt {
                unsafe {
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
                child.mmap_bump = parent.mmap_bump; 
                
                // 1. Share memory frames (CoW implementation)
                crate::memory::clone_user_address_space(parent.cr3, child.cr3);

                // 🚨 CoW FIX: Flush the Parent's TLB!
                // Since we just marked the parent's active pages as Read-Only, we MUST 
                // force the CPU to forget the old Writable cached pages immediately.
                unsafe {
                    let cr3 = x86_64::registers::control::Cr3::read();
                    x86_64::registers::control::Cr3::write(cr3.0, cr3.1); 
                }

                // 2. Clone file descriptors (Sockets, files, pipes)
                for i in 0..32 {
                    if let Some(fd) = &parent.fd_table[i] {
                        child.fd_table[i] = Some(fd.clone());
                    }
                }
            }

            // 3. Setup the child's return stack frame
            let stack_top = child.kernel_stack_top;
            let iretq_ptr = stack_top - 40;

            unsafe {
                let iret_slice = core::slice::from_raw_parts_mut(iretq_ptr as *mut u64, 5);
                iret_slice[0] = frame.rcx;         
                iret_slice[1] = 0x33;              
                iret_slice[2] = frame.r11 | 0x200; 
                iret_slice[3] = frame.user_rsp;   
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
                regs[14] = 0; // 🚨 The child receives PID 0 as its return value!
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
            
            // 4. The parent process receives the child's actual PID!
            frame.rax = child.pid;
            
            percpu.scheduler.tasks.push(child);
        },
        58 => { // SYS_SPAWN_THREAD
            let entry_point = arg1;
            let user_stack = arg2;

            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            if curr_idx >= percpu.scheduler.tasks.len() { frame.rax = ENOSYS as u64; return; }
            
            let parent_cr3 = percpu.scheduler.tasks[curr_idx].cr3;
            let mut thread = crate::process::Process::new_thread(parent_cr3).expect("Failed to spawn thread");
            
            {
                let parent = &percpu.scheduler.tasks[curr_idx];
                thread.parent_pid = Some(parent.pid);
                thread.mmap_bump = parent.mmap_bump;

                // Share the File Descriptors (Sockets)
                for i in 0..32 {
                    if let Some(fd) = &parent.fd_table[i] {
                        thread.fd_table[i] = Some(fd.clone());
                    }
                }
            }

            let stack_top = thread.kernel_stack_top;
            let iretq_ptr = stack_top - 40;

            unsafe {
                let iret_slice = core::slice::from_raw_parts_mut(iretq_ptr as *mut u64, 5);
                iret_slice[0] = entry_point;       // RIP: Where the thread starts executing
                iret_slice[1] = 0x33;              // CS: Userspace Code Segment
                iret_slice[2] = frame.r11 | 0x200; // RFLAGS: Enable Interrupts
                iret_slice[3] = user_stack;        // RSP: The custom stack we allocated for the thread
                iret_slice[4] = 0x2B;              // SS: Userspace Stack Segment
            }

            let regs_ptr = iretq_ptr - 120;
            unsafe {
                core::ptr::write_bytes(regs_ptr as *mut u8, 0, 120); // Zero out general registers
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

            thread.saved_rsp = final_rsp;
            frame.rax = thread.pid;
            
            // --- THE TRUE SMP LOAD BALANCER ---
            unsafe {
                let active_cores = crate::smp::ACTIVE_CORES.load(core::sync::atomic::Ordering::SeqCst);
                let mut target_core = percpu.logical_id;
                let mut min_tasks = usize::MAX;

                // 1. Scan all active CPU cores
                if let Some(all_cores) = &mut crate::percpu::PER_CPU {
                    for i in 0..active_cores {
                        let count = all_cores[i].scheduler.tasks.len();
                        
                        // 2. Find the core with the lightest workload
                        if count < min_tasks {
                            min_tasks = count;
                            target_core = i;
                        }
                    }
                    
                    crate::serial_println!("[SMP] Load Balancer: Offloading Thread to Core {} (Tasks: {})", target_core, min_tasks);
                    
                    // 3. Inject the thread directly into the idle core's hardware queue!
                    all_cores[target_core].scheduler.tasks.push(thread);
                }
            }
            // ----------------------------------
        },
          
       
        59 => { // sys_execve
            let ptr = arg1 as *const u8;
            let len = arg2 as usize;
            
            // 1. Copy the path to a safe Kernel String BEFORE shredding user memory!
            let path_str = if let Ok(s) = core::str::from_utf8(unsafe { core::slice::from_raw_parts(ptr, len) }) {
                alloc::string::String::from(s.trim_matches(char::from(0)).trim())
            } else {
                frame.rax = (-1i64) as u64;
                return;
            };

            // 2. Read the file using the safe Kernel String
            if let Some(elf_data) = crate::vfs::VFS.read_file_alloc(&path_str) {
                let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                let task = &mut percpu.scheduler.tasks[curr_idx];
                
                // 3. Shred the old memory
                crate::memory::clear_user_address_space(task.cr3);
                
                // 🚨 THE FIX: Reset the bump allocator to a VALID canonical address! 🚨
                // 0x1000_0000_0000 is safely inside the lower user half.
                task.mmap_bump = 0x1000_0000_0000;
                
                // 4. Flush the CPU TLB
                unsafe {
                    let cr3 = x86_64::registers::control::Cr3::read();
                    x86_64::registers::control::Cr3::write(cr3.0, cr3.1);
                }
                
                // 5. Load the new ELF
                if let Ok(entry_point) = crate::process::load_elf(&elf_data) {
                    let stack_base = 0x7FFF_0000_0000;
                    let stack_pages = 32; 
                    if crate::memory::allocate_user_pages_at(stack_base, stack_pages).is_ok() {
                        let stack_top = ((stack_base + (stack_pages as u64 * 4096)) & !0xF) - 8; 
                        
                        // Override the Syscall Return Frame!
                        frame.rcx = entry_point;    // Jump to the new App's _start
                        frame.user_rsp = stack_top; // Give it the fresh stack
                        
                        // 🚨 SECURITY FIX: Zero out ALL general purpose registers!
                        // This prevents the new app from inheriting garbage state from the old app.
                        frame.rdi = 0; frame.rsi = 0; frame.rdx = 0; frame.rbp = 0;
                        frame.r8 = 0; frame.r9 = 0; frame.r10 = 0; 
                        frame.r11 = 0x202; // RFLAGS: Ensure hardware interrupts stay enabled!
                        frame.r12 = 0; frame.r13 = 0; frame.r14 = 0; frame.r15 = 0;
                        frame.rbx = 0;

                        // 6. Safely update the task name for the System Monitor
                        let mut name_arr = [0u8; 16];
                        let bytes = path_str.as_bytes();
                        let copy_len = core::cmp::min(16, bytes.len());
                        name_arr[..copy_len].copy_from_slice(&bytes[..copy_len]);
                        task.name = name_arr;
                        
                        frame.rax = 0; // Success
                        return;        // Bypass default block exit
                    }
                }
            }
            frame.rax = (-1i64) as u64; // File Not Found or Parse Error
        },

        60 => { // SYS_EXIT
            // 0. DISABLE INTERRUPTS to prevent getting buried alive!
            x86_64::instructions::interrupts::disable();

            let exit_code = arg1 as i64;
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            let task = &mut percpu.scheduler.tasks[curr_idx];
            
            crate::serial_println!("[PID {}] Exited (Code: {})", task.pid, exit_code);
            
            // 1. Safe FD Teardown using Arc Reference Counting
            for i in 0..32 { 
                if let Some(FileDescriptor::Socket(sock_mtx)) = &task.fd_table[i] {
                    if alloc::sync::Arc::strong_count(sock_mtx) == 1 {
                        let sock = sock_mtx.lock();
                        if let Some(sockets) = crate::drivers::net::GLOBAL_SOCKETS.lock().as_mut() {
                            match sock.kind {
                                SocketKind::Tcp(handle) => {
                                    let socket = sockets.get_mut::<smoltcp::socket::tcp::Socket>(handle);
                                    socket.abort(); // Send TCP RST
                                    sockets.remove(handle); // Free the memory
                                },
                                SocketKind::Udp(handle) => {
                                    sockets.remove(handle);
                                }
                            }
                        }
                    }
                }
                // Safely drop our reference to the FD
                task.fd_table[i] = None; 
            }

            // 2. Shred ONLY the user memory tables securely. 
            // DO NOT swap CR3 to KERNEL_CR3, or the CPU will instantly Triple Fault when trying to use the stack!
            crate::memory::clear_user_address_space(task.cr3);

            // 3. Mark as Zombie at the VERY END, once all locks are released
            task.state = crate::scheduler::TaskState::Zombie;
            
            // 4. Re-enable interrupts and wait for the scheduler to context-switch away natively
            unsafe {
                x86_64::instructions::interrupts::enable();
                loop { core::arch::asm!("hlt") }
            }
        },

        131 => { frame.rax = 0; }, // SYS_SIGALTSTACK

        158 => { // SYS_ARCH_PRCTL (TLS Support)
            let code = arg1;
            let addr = arg2;
            if code == 0x1002 { // ARCH_SET_FS
                unsafe { x86_64::registers::model_specific::FsBase::write(x86_64::VirtAddr::new(addr)); }
                frame.rax = 0;
            } else {
                frame.rax = EINVAL as u64;
            }
        },

        218 => { frame.rax = 1; }, // SYS_SET_TID_ADDRESS

        318 => { // SYS_GETRANDOM (Required for Rust HashMaps)
            let buf_ptr = arg1 as *mut u8;
            let len = arg2 as usize;
            if is_valid_user_ptr(buf_ptr, len) {
                unsafe { core::ptr::write_bytes(buf_ptr, 42, len); }
                frame.rax = len as u64;
            } else {
                frame.rax = EFAULT as u64;
            }
        },
        
        // --- CUSTOM NYXOS SYSCALLS ---
        501 => {
              unsafe {
                 // Convert your color index to a raw 32-bit hex color for the GPU
                 let raw_color = match arg5 as u8 {
                     0 => 0xFF000000, // Black
                     1 => 0xFF0000FF, // Blue
                     2 => 0xFF00FF00, // Green
                     3 => 0xFF00FFFF, // Cyan
                     4 => 0xFFFF0000, // Red
                     14 => 0xFFFFFF00, // Yellow
                     _ => 0xFFFFFFFF, // White
                 };

                 let mut hardware_accelerated = false;

                 // Try to use the GPU first!
                 if let Some(gpu) = crate::drivers::gpu::intel::INTEL_GPU.lock().as_mut() {
                     if let Some(p) = &crate::gui::SCREEN_PAINTER {
                         let screen_w = p.info.width as u32;
                         let screen_h = p.info.height as u32;
                         let pitch = (p.info.stride * 4) as u32;
                         
                         let start_x = core::cmp::min(arg1 as u32, screen_w);
                         let start_y = core::cmp::min(arg2 as u32, screen_h);
                         let max_w = screen_w.saturating_sub(start_x);
                         let max_h = screen_h.saturating_sub(start_y);
                         let w = core::cmp::min(arg3 as u32, max_w);
                         let h = core::cmp::min(arg4 as u32, max_h);

                         gpu.fill_rect(start_x, start_y, w, h, raw_color, pitch);
                         hardware_accelerated = true;
                     }
                 }

                 // CPU Fallback (If GPU is offline or not Intel)
                 if !hardware_accelerated {
                     if let Some(p) = &mut crate::gui::SCREEN_PAINTER {
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
             }
        },

        502 => { // sys_swap_buffers
             unsafe {
                 if let Some(gpu) = crate::drivers::gpu::intel::INTEL_GPU.lock().as_mut() {
                     // Because SCREEN_PAINTER is a `static mut`, reading it MUST be inside the unsafe block
                     if let Some(p) = &crate::gui::SCREEN_PAINTER {
                         let w = p.info.width as u32;
                         let h = p.info.height as u32;
                         let pitch = (p.info.stride * 4) as u32;
                         
                         // Copy from GPU Backbuffer (0x900000) to Real Screen (0x100000)
                         gpu.copy_rect(
                             0, 0, pitch, 0x900000, 
                             0, 0, pitch, 0x100000, 
                             w, h
                         );
                     }
                 }
             }
        },

        503 => { // sys_gpu_sync
             unsafe {
                 if let Some(gpu) = crate::drivers::gpu::intel::INTEL_GPU.lock().as_mut() {
                     gpu.wait_for_idle();
                 }
             }
        },

        504 => { 
            // Return true uptime, completely immune to CPU frequency scaling!
            frame.rax = crate::time::UPTIME_MS.load(core::sync::atomic::Ordering::Relaxed); 
        },

        
        505 => { 
            // THE FIX: Shield the spinlock from hardware interrupts!
            // This prevents IRQ 12 from firing while we are reading the mouse state.
            let m_val = x86_64::instructions::interrupts::without_interrupts(|| {
                let m = crate::mouse::MOUSE_STATE.lock();
                (m.x as u64) << 32 | (m.y as u64) << 16 | (if m.left_click {1} else {0}) << 1 | (if m.right_click {1} else {0})
            });
            frame.rax = m_val;
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
                if let Some(p) = &mut crate::gui::SCREEN_PAINTER {
                    let virt_start = p.buffer.as_ptr() as u64;
                    if let Some(phys) = crate::memory::virt_to_phys(virt_start) {
                        // THE FIX: p.buffer.len() is ALREADY in bytes! Do not multiply by 4!
                        if let Ok(user_virt) = crate::memory::map_user_framebuffer(phys, p.buffer.len() as u64) {
                            frame.rax = user_virt;
                        } else { frame.rax = 0; }
                    } else { frame.rax = 0; }
                } else { frame.rax = 0; }
            }
        },
        // -----------------------------------------------------
        // VFS DIRECTORY LISTING SYSCALLS
        // -----------------------------------------------------
        
        // Syscall 510: Get Directory Item Count
        510 => {
            let path_ptr = arg1 as *const u8;
            let path_len = arg2 as usize;
            
            // 🔥 FIX: Wrap raw slice creation in an unsafe block
            let path_slice = unsafe { core::slice::from_raw_parts(path_ptr, path_len) };
            
            if let Ok(path) = core::str::from_utf8(path_slice) {
                let list = crate::vfs::VFS.list_dir(path);
                frame.rax = list.len() as u64;
            } else {
                frame.rax = 0;
            }
        }
        
        // Syscall 511: Get Directory Item String by Index
        511 => {
            let index = arg1 as usize;
            let buf_ptr = arg2 as *mut u8;
            let path_ptr = arg3 as *const u8;
            let path_len = arg4 as usize;
            
            // 🔥 FIX: Wrap raw slice creation in an unsafe block
            let path_slice = unsafe { core::slice::from_raw_parts(path_ptr, path_len) };
            
            if let Ok(path) = core::str::from_utf8(path_slice) {
                let list = crate::vfs::VFS.list_dir(path);
                
                if let Some(entry) = list.get(index) {
                    let bytes = entry.as_bytes();
                    
                    // 🔥 FIX: Wrap the memory copy in an unsafe block
                    unsafe {
                        core::ptr::copy_nonoverlapping(bytes.as_ptr(), buf_ptr, bytes.len());
                    }
                    
                    frame.rax = bytes.len() as u64;
                } else {
                    frame.rax = 0;
                }
            } else {
                frame.rax = 0;
            }
        }
        513 => { // sys_wait_vsync
            unsafe {
                if let Some(gpu) = crate::drivers::gpu::intel::INTEL_GPU.lock().as_mut() {
                    gpu.wait_for_vsync();
                }
            }
            frame.rax = 0;
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
        524 => { 
            // SYSCALL 524: sys_get_system_info
            let info_ptr = arg1 as *mut SystemInfo;
            
            // SECURITY: Prevent Userspace from tricking the Kernel into overwriting Ring 0 memory!
            if !is_valid_user_ptr(info_ptr as *const u8, core::mem::size_of::<SystemInfo>()) {
                frame.rax = EFAULT as u64;
                return;
            }
            
            unsafe {
                // 1. Thermal Telemetry
                let temp = crate::thermal::get_intel_silicon_temp();
                (*info_ptr).current_temp = temp;
                (*info_ptr).active_cooling = if temp >= 75 { 1 } else { 0 };
                
                // 2. Hardware Fan Telemetry (SMM)
                (*info_ptr).cpu_fan_rpm = crate::laptop_fans::get_dell_fan_rpm(0);
                (*info_ptr).gpu_fan_rpm = crate::laptop_fans::get_dell_fan_rpm(1);
                
                // 3. Task Scheduler Telemetry
                let mut count = 0;
                if let Some(cores) = &crate::percpu::PER_CPU {
                    for core in cores.iter() {
                        for task in core.scheduler.tasks.iter() {
                            if task.cpu_ticks > 0 || task.state == crate::scheduler::TaskState::Running {
                                if count < 64 {
                                    (*info_ptr).tasks[count] = TaskInfo {
                                        pid: task.pid,
                                        cpu_ticks: task.cpu_ticks,
                                        state: task.state as u8,
                                        name: task.name,
                                    };
                                    count += 1;
                                }
                            }
                        }
                    }
                }
                (*info_ptr).task_count = count as u64;
            }
            frame.rax = 0;
        },
        525 => { 
            // SYSCALL 525: sys_sleep_ms (THE SELF-HEALING FIX)
            let ms = arg1 as u64;
            let wake_ms = crate::time::UPTIME_MS.load(core::sync::atomic::Ordering::Relaxed) + ms; 
            
            unsafe {
                // 1. We MUST re-enable interrupts so the APIC timer can tick while we sleep!
                x86_64::instructions::interrupts::enable();
                
                loop {
                    let percpu = crate::percpu::current();
                    let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                    {
                        let task = &mut percpu.scheduler.tasks[curr_idx];
                        task.state = crate::scheduler::TaskState::Blocked;
                        task.wake_tsc = wake_ms; 
                    }
                    
                    // 2. Yield the CPU
                    core::arch::asm!("int 0x41"); 
                    
                    // 3. When we wake up, check WHY we woke up
                    let percpu = crate::percpu::current();
                    let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                    let task = &mut percpu.scheduler.tasks[curr_idx];
                    
                    if task.wake_tsc == 0 { break; } // Human Input Override (Mouse Touched!)
                    if crate::time::UPTIME_MS.load(core::sync::atomic::Ordering::Relaxed) >= wake_ms { break; } // Time passed!
                    
                    // 4. If we woke up illegally (scheduler fallback), HALT to save battery!
                    x86_64::instructions::hlt(); 
                }
                
                // 5. Safely disable interrupts before returning to the syscall dispatcher
                x86_64::instructions::interrupts::disable();
            }
            frame.rax = 0;
        },

        526 => { 
            let buf_ptr = arg1 as *mut u8;
            let max_len = arg2 as usize;
            
            if !is_valid_user_ptr(buf_ptr, max_len) { 
                frame.rax = EFAULT as u64; 
                return; 
            }
            frame.rax = crate::acpi::get_dsdt_data(buf_ptr, max_len) as u64;
        },

        530 => { // SYS_CREATE_SHM
            let size = arg1 as usize;
            if let Some(id) = crate::memory::create_shm_block(size) {
                frame.rax = id;
            } else { frame.rax = 0; }
        },

        531 => { // SYS_MAP_SHM
            let shm_id = arg1;
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            let task = &mut percpu.scheduler.tasks[curr_idx];
            
            // Align the bump allocator to a 2MB boundary to ensure SHM never overlaps with normal heap
            let target_addr = (task.mmap_bump + 0x1FFFFF) & !0x1FFFFF; 
            
            let size = {
                let reg = crate::memory::SHM_REGISTRY.lock();
                if let Some(b) = reg.iter().find(|b| b.id == shm_id) { b.size } else { 0 }
            };
            
            if size > 0 {
                let num_pages = (size + 0xFFF) / 0x1000;
                task.mmap_bump = target_addr + ((num_pages as u64) * 0x1000); 
                
                if let Ok(vaddr) = crate::memory::map_shm_block(shm_id, target_addr) {
                    frame.rax = vaddr;
                } else { frame.rax = 0; }
            } else { frame.rax = 0; }
        },
        532 => { // SYS_IPC_SEND
            let target_pid = arg1;
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            let sender_pid = percpu.scheduler.tasks[curr_idx].pid;
            
            let msg = crate::process::IpcMessage {
                sender_pid, msg_type: arg2, data1: arg3, data2: arg4,
            };
            
            let mut found = false;
            unsafe {
                let active_cores = crate::smp::ACTIVE_CORES.load(core::sync::atomic::Ordering::SeqCst);
                if let Some(cores) = &mut crate::percpu::PER_CPU {
                    for i in 0..active_cores {
                        for task in cores[i].scheduler.tasks.iter_mut() {
                            if task.pid == target_pid {
                                task.mailbox.push_back(msg);
                                // If the task was sleeping forever waiting for IPC, wake it up!
                                if task.state == crate::scheduler::TaskState::Blocked && task.wake_tsc == u64::MAX {
                                    task.state = crate::scheduler::TaskState::Ready;
                                    task.wake_tsc = 0;
                                }
                                found = true;
                                break;
                            }
                        }
                        if found { break; }
                    }
                }
            }
            frame.rax = if found { 1 } else { 0 }; 
        },

        533 => { 
            // SYSCALL 533: sys_ipc_recv
            let msg_ptr = arg1 as *mut crate::process::IpcMessage;
            let block = arg2 == 1;
            
            if !is_valid_user_ptr(msg_ptr as *const u8, core::mem::size_of::<crate::process::IpcMessage>()) {
                frame.rax = EFAULT as u64; return;
            }
            
            if block {
                unsafe {
                    // Re-enable interrupts to prevent timer deadlocks
                    x86_64::instructions::interrupts::enable();
                    loop {
                        let percpu = crate::percpu::current();
                        let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                        let task = &mut percpu.scheduler.tasks[curr_idx];
                        
                        if let Some(msg) = task.mailbox.pop_front() {
                            *msg_ptr = msg;
                            frame.rax = 1;
                            break;
                        }
                        
                        task.state = crate::scheduler::TaskState::Blocked;
                        task.wake_tsc = u64::MAX; 
                        
                        core::arch::asm!("int 0x41"); 
                        
                        let percpu = crate::percpu::current();
                        let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                        let task = &mut percpu.scheduler.tasks[curr_idx];
                        
                        if task.mailbox.is_empty() {
                            x86_64::instructions::hlt();
                        }
                    }
                    x86_64::instructions::interrupts::disable();
                }
            } else {
                let percpu = crate::percpu::current();
                let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                let task = &mut percpu.scheduler.tasks[curr_idx];
                if let Some(msg) = task.mailbox.pop_front() {
                    unsafe { *msg_ptr = msg; }
                    frame.rax = 1; 
                } else {
                    frame.rax = 0; 
                }
            }
        },
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
                    match sock.kind {
                        SocketKind::Udp(handle) => {
                            let socket = sockets.get_mut::<smoltcp::socket::udp::Socket>(handle);
                            if let Ok((data, _meta)) = socket.recv() {
                                let copy_len = core::cmp::min(data.len(), len);
                                unsafe { core::ptr::copy_nonoverlapping(data.as_ptr(), buf_ptr, copy_len); }
                                return copy_len as isize;
                            }
                        },
                        SocketKind::Tcp(handle) => {
                            let socket = sockets.get_mut::<smoltcp::socket::tcp::Socket>(handle);
                            if socket.can_recv() {
                                let user_slice = unsafe { core::slice::from_raw_parts_mut(buf_ptr, len) };
                                if let Ok(received) = socket.recv_slice(user_slice) {
                                    if received > 0 { return received as isize; }
                                }
                            } else if !socket.may_recv() {
                                return 0; 
                            }
                        }
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

    if let Some(fd_enum) = &task.fd_table[fd] {
        match fd_enum {
            FileDescriptor::File(open_file) => return open_file.write(buf_slice) as isize,
            FileDescriptor::Socket(sock_mtx) => {
                crate::drivers::net::poll_network(); 

                let sock = sock_mtx.lock();
                let mut sockets_lock = crate::drivers::net::GLOBAL_SOCKETS.lock();
                
                if let Some(sockets) = sockets_lock.as_mut() {
                    match sock.kind {
                        SocketKind::Udp(handle) => {
                            let socket = sockets.get_mut::<smoltcp::socket::udp::Socket>(handle);
                            if let Some(endpoint) = sock.remote {
                                if socket.send_slice(buf_slice, endpoint).is_ok() {
                                    return buf_slice.len() as isize;
                                }
                            }
                        },
                        SocketKind::Tcp(handle) => {
                            let socket = sockets.get_mut::<smoltcp::socket::tcp::Socket>(handle);
                            if socket.can_send() {
                                if let Ok(sent) = socket.send_slice(buf_slice) {
                                    return sent as isize;
                                }
                            }
                        }
                    }
                }
                return EAGAIN as isize;
            },
            FileDescriptor::PipeWrite(pipe_mtx) => {
                let mut pipe = pipe_mtx.lock();
                for &b in buf_slice { pipe.push_back(b); }
                return len as isize;
            },
            FileDescriptor::PipeRead(_) => return EBADF as isize,
        }
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
        let local_port = NEXT_LOCAL_PORT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        
        let handle = if _typ == 1 {
            let rx_buffer = smoltcp::socket::tcp::SocketBuffer::new(alloc::vec![0; 32768]);
            let tx_buffer = smoltcp::socket::tcp::SocketBuffer::new(alloc::vec![0; 32768]);
            let socket = smoltcp::socket::tcp::Socket::new(rx_buffer, tx_buffer);
            sockets.add(socket)
        } else {
            let rx_buffer = smoltcp::socket::udp::PacketBuffer::new(alloc::vec![smoltcp::socket::udp::PacketMetadata::EMPTY; 8], alloc::vec![0; 2048]);
            let tx_buffer = smoltcp::socket::udp::PacketBuffer::new(alloc::vec![smoltcp::socket::udp::PacketMetadata::EMPTY; 8], alloc::vec![0; 2048]);
            let mut socket = smoltcp::socket::udp::Socket::new(rx_buffer, tx_buffer);
            let _ = socket.bind(local_port);
            sockets.add(socket)
        };

        let kind = if _typ == 1 { SocketKind::Tcp(handle) } else { SocketKind::Udp(handle) };
        let ks = KernelSocket { kind, local_port, remote: None };
        
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
            
            if let SocketKind::Tcp(handle) = sock.kind {
                let mut sockets_lock = crate::drivers::net::GLOBAL_SOCKETS.lock();
                let mut iface_lock = crate::drivers::net::NET_IFACE.lock();
                
                if let (Some(sockets), Some(iface)) = (sockets_lock.as_mut(), iface_lock.as_mut()) {
                    let socket = sockets.get_mut::<smoltcp::socket::tcp::Socket>(handle);
                    let local_port = sock.local_port;
                    
                    if socket.connect(iface.context(), IpEndpoint::new(addr, port), local_port).is_err() {
                        return -11; // EAGAIN
                    }
                }
            }
            
            crate::drivers::net::poll_network();
            return 0; 
        }
    }
    EBADF
}