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
            
            // REMOVE the old ethernet_interrupt_stub line from inside the unsafe block
        }
        
        // new x86-interrupt handler directly to slot 0x30 (48) outside the unsafe block
        idt[0x30].set_handler_fn(rtl8168_interrupt_handler);
        
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

/// Convert the RTC's packed wall-clock (see rtc::read_packed) into Unix epoch seconds.
/// Packed layout: [7:0]=sec [15:8]=min [23:16]=hour [31:24]=day [39:32]=month [63:40]=year(full).
/// Uses Howard Hinnant's days-from-civil algorithm (proleptic Gregorian, UTC — no timezone).
fn rtc_packed_to_unix(p: u64) -> i64 {
    let sec = (p & 0xFF) as i64;
    let min = ((p >> 8) & 0xFF) as i64;
    let hour = ((p >> 16) & 0xFF) as i64;
    let day = ((p >> 24) & 0xFF) as i64;
    let month = ((p >> 32) & 0xFF) as i64;
    let year = ((p >> 40) & 0xFFFFFF) as i64;
    if year == 0 || month == 0 || day == 0 { return 0; } // RTC not readable yet

    let y = if month <= 2 { year - 1 } else { year };
    let era = (if y >= 0 { y } else { y - 399 }) / 400;
    let yoe = y - era * 400;                                   // [0, 399]
    let doy = (153 * (if month > 2 { month - 3 } else { month + 9 }) + 2) / 5 + day - 1; // [0, 365]
    let doe = yoe * 365 + yoe / 4 - yoe / 100 + doy;           // [0, 146096]
    let days = era * 146097 + doe - 719468;                    // days since 1970-01-01
    days * 86400 + hour * 3600 + min * 60 + sec
}

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
    let arg6 = frame.r9;

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
                if let Ok(loaded) = crate::process::load_elf_full(&elf_data) {
                    let entry_point = loaded.entry;
                    let stack_base = 0x7FFF_0000_0000;
                    let stack_pages = 32;
                    if crate::memory::allocate_user_pages_at(stack_base, stack_pages).is_ok() {
                        let stack_top = ((stack_base + (stack_pages as u64 * 4096)) & !0xF) - 8;

                        // B2: build the SysV entry stack (argc/argv/envp/auxv) in the freshly loaded
                        // address space so a std runtime can start. We're already on the task's CR3.
                        let entry_rsp = unsafe {
                            crate::process::build_initial_stack(stack_top, &path_str, &loaded)
                        };

                        // Override the Syscall Return Frame!
                        frame.rcx = entry_point;    // Jump to the new App's _start
                        frame.user_rsp = entry_rsp; // Give it the fresh SysV stack
                        
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

        60 | 231 => { // SYS_EXIT / SYS_EXIT_GROUP (single-threaded teardown covers both here)
            // 0. DISABLE INTERRUPTS to prevent getting buried alive!
            x86_64::instructions::interrupts::disable();

            let exit_code = arg1 as i64;
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            let task = &mut percpu.scheduler.tasks[curr_idx];

            let my_cr3 = task.cr3;
            let my_pid = task.pid;
            crate::serial_println!("[PID {}] Exited (Code: {})", my_pid, exit_code);
            task.exit_code = exit_code; // B1: retained for the parent's wait4 before reaping.

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

            // B-β.2a: threads share `cr3`. Count OTHER live tasks in this address space across all
            // cores; if any exist, this is a *thread* exit — we must NOT shred the shared user memory
            // (that would kill the siblings). Only the sole owner of the cr3 tears the memory down.
            let mut siblings_alive = false;
            unsafe {
                let active = crate::smp::ACTIVE_CORES.load(core::sync::atomic::Ordering::SeqCst);
                if let Some(cores) = &mut crate::percpu::PER_CPU {
                    'scan: for i in 0..active {
                        for t in cores[i].scheduler.tasks.iter() {
                            if t.pid != my_pid
                                && t.cr3 == my_cr3
                                && t.state != crate::scheduler::TaskState::Empty
                                && t.state != crate::scheduler::TaskState::Zombie {
                                siblings_alive = true;
                                break 'scan;
                            }
                        }
                    }
                }
            }

            let task = &mut percpu.scheduler.tasks[curr_idx];
            if siblings_alive {
                // 2a. Thread exit: leave the shared address space intact. Self-reap this slot
                // (Empty), so no wait4 is required for a joined worker. The kernel stack leaks,
                // consistent with the existing process-exit path (which also never frees it).
                task.state = crate::scheduler::TaskState::Empty;
            } else {
                // 2b. Sole owner: shred ONLY the user memory tables securely.
                // DO NOT swap CR3 to KERNEL_CR3, or the CPU will instantly Triple Fault using the stack!
                crate::memory::clear_user_address_space(my_cr3);
                // 3. Mark as Zombie at the VERY END, once all locks are released, for the parent's wait4.
                task.state = crate::scheduler::TaskState::Zombie;
            }

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
                // B1: fill from a TSC/uptime-mixed xorshift instead of a constant, so std's
                // HashMap RandomState (and any getrandom() user) gets non-repeating bytes. Not
                // cryptographically strong, but adequate for hash-DoS resistance on this target.
                let mut seed = {
                    let tsc: u64;
                    unsafe { core::arch::asm!("rdtsc", out("rax") tsc, out("rdx") _, options(nomem, nostack)); }
                    tsc ^ crate::time::UPTIME_MS.load(core::sync::atomic::Ordering::Relaxed).wrapping_mul(0x9E37_79B9_7F4A_7C15)
                };
                let mut i = 0;
                while i < len {
                    // xorshift64*
                    seed ^= seed >> 12; seed ^= seed << 25; seed ^= seed >> 27;
                    let r = seed.wrapping_mul(0x2545_F491_4F6C_DD1D);
                    let n = core::cmp::min(8, len - i);
                    unsafe { core::ptr::copy_nonoverlapping(r.to_le_bytes().as_ptr(), buf_ptr.add(i), n); }
                    i += n;
                }
                frame.rax = len as u64;
            } else {
                frame.rax = EFAULT as u64;
            }
        },

        // ====================================================================
        // std-port syscalls (B1): the Linux-numbered primitives std needs that
        // were previously falling through to the `_ => EINVAL` arm below.
        // ====================================================================
        24 => { // SYS_SCHED_YIELD — give up the rest of this quantum.
            unsafe { core::arch::asm!("int 0x41"); }
            frame.rax = 0;
        },

        39 | 186 => { // SYS_GETPID / SYS_GETTID (threads carry their own pid here).
            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
            frame.rax = percpu.scheduler.tasks[curr_idx].pid;
        },

        228 => { // SYS_CLOCK_GETTIME(clockid, *timespec{tv_sec:i64, tv_nsec:i64})
            let clockid = arg1 as i32;
            let ts_ptr = arg2 as *mut i64;
            if !is_valid_user_ptr(ts_ptr as *const u8, 16) { frame.rax = EFAULT as u64; return; }
            let (sec, nsec) = match clockid {
                0 => (rtc_packed_to_unix(crate::rtc::read_packed()), 0i64), // CLOCK_REALTIME
                _ => { // CLOCK_MONOTONIC / BOOTTIME / everything else -> uptime
                    let ms = crate::time::UPTIME_MS.load(core::sync::atomic::Ordering::Relaxed);
                    ((ms / 1000) as i64, ((ms % 1000) * 1_000_000) as i64)
                }
            };
            unsafe { *ts_ptr = sec; *ts_ptr.add(1) = nsec; }
            frame.rax = 0;
        },

        35 | 230 => { // SYS_NANOSLEEP(*req,*rem) / SYS_CLOCK_NANOSLEEP(clockid,flags,*req,*rem)
            let req_ptr = (if id == 35 { arg1 } else { arg3 }) as *const i64;
            if !is_valid_user_ptr(req_ptr as *const u8, 16) { frame.rax = EFAULT as u64; return; }
            let (sec, nsec) = unsafe { (*req_ptr, *req_ptr.add(1)) };
            // Round up to whole milliseconds (the scheduler's tick granularity).
            let ms = (sec.max(0) as u64) * 1000 + ((nsec.max(0) as u64) + 999_999) / 1_000_000;
            if ms > 0 {
                let wake_ms = crate::time::UPTIME_MS.load(core::sync::atomic::Ordering::Relaxed) + ms;
                unsafe {
                    x86_64::instructions::interrupts::enable();
                    loop {
                        let percpu = crate::percpu::current();
                        let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                        percpu.scheduler.tasks[curr_idx].state = crate::scheduler::TaskState::Blocked;
                        percpu.scheduler.tasks[curr_idx].wake_tsc = wake_ms;
                        core::arch::asm!("int 0x41");
                        if crate::time::UPTIME_MS.load(core::sync::atomic::Ordering::Relaxed) >= wake_ms { break; }
                        x86_64::instructions::hlt();
                    }
                    x86_64::instructions::interrupts::disable();
                }
            }
            frame.rax = 0;
        },

        202 => { // SYS_FUTEX(uaddr, op, val, timeout, uaddr2, val3)
            const FUTEX_WAIT: u32 = 0;
            const FUTEX_WAKE: u32 = 1;
            const FUTEX_PRIVATE_FLAG: u32 = 128;
            const FUTEX_CLOCK_REALTIME: u32 = 256;
            let uaddr = arg1;
            let cmd = (arg2 as u32) & !(FUTEX_PRIVATE_FLAG | FUTEX_CLOCK_REALTIME);
            let val = arg3 as u32;

            if !is_valid_user_ptr(uaddr as *const u8, 4) { frame.rax = EFAULT as u64; return; }
            let cr3 = {
                let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                percpu.scheduler.tasks[curr_idx].cr3
            };

            match cmd {
                FUTEX_WAIT => {
                    // Optional relative timeout (timespec at arg4). None => wait indefinitely.
                    let deadline = if arg4 != 0 && is_valid_user_ptr(arg4 as *const u8, 16) {
                        let tp = arg4 as *const i64;
                        let (s, n) = unsafe { (*tp, *tp.add(1)) };
                        let ms = (s.max(0) as u64) * 1000 + ((n.max(0) as u64) + 999_999) / 1_000_000;
                        Some(crate::time::UPTIME_MS.load(core::sync::atomic::Ordering::Relaxed) + ms)
                    } else { None };

                    unsafe {
                        x86_64::instructions::interrupts::enable();
                        let mut ret = 0i64;
                        loop {
                            // Durable re-check: the waker stores a new value at *uaddr BEFORE waking.
                            // Reading it each iteration means a lost FUTEX_WAKE cannot hang us forever.
                            let cur = core::ptr::read_volatile(uaddr as *const u32);
                            if cur != val { ret = 0; break; }                   // value changed -> woken
                            let now = crate::time::UPTIME_MS.load(core::sync::atomic::Ordering::Relaxed);
                            if let Some(d) = deadline { if now >= d { ret = -110; break; } } // ETIMEDOUT

                            // Block, but cap the sleep at 10 ms so a missed wake self-heals into a
                            // re-poll of *uaddr (liveness insurance for this lock-free wake path).
                            let repoll = now + 10;
                            let cap = match deadline { Some(d) => core::cmp::min(d, repoll), None => repoll };
                            let percpu = crate::percpu::current();
                            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                            percpu.scheduler.tasks[curr_idx].state = crate::scheduler::TaskState::Blocked;
                            percpu.scheduler.tasks[curr_idx].wake_tsc = cap;
                            percpu.scheduler.tasks[curr_idx].futex_addr = uaddr;

                            core::arch::asm!("int 0x41");

                            let percpu = crate::percpu::current();
                            let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                            percpu.scheduler.tasks[curr_idx].futex_addr = 0;
                        }
                        x86_64::instructions::interrupts::disable();
                        frame.rax = ret as u64;
                    }
                },
                FUTEX_WAKE => {
                    // Wake up to `val` waiters parked on this (cr3, uaddr). Scan every core's run queue.
                    let mut woken = 0u32;
                    let want = val;
                    unsafe {
                        let active = crate::smp::ACTIVE_CORES.load(core::sync::atomic::Ordering::SeqCst);
                        if let Some(cores) = &mut crate::percpu::PER_CPU {
                            'outer: for i in 0..active {
                                for task in cores[i].scheduler.tasks.iter_mut() {
                                    if woken >= want { break 'outer; }
                                    if task.futex_addr == uaddr && task.cr3 == cr3
                                        && task.state == crate::scheduler::TaskState::Blocked {
                                        task.state = crate::scheduler::TaskState::Ready;
                                        task.wake_tsc = 0;
                                        task.futex_addr = 0;
                                        woken += 1;
                                    }
                                }
                            }
                        }
                    }
                    frame.rax = woken as u64;
                },
                _ => { frame.rax = EINVAL as u64; }
            }
        },

        61 => { // SYS_WAIT4(pid, *wstatus, options, *rusage) — reap a Zombie child.
            const WNOHANG: u64 = 1;
            let want_pid = arg1 as i64;   // -1 = any child
            let wstatus = arg2 as *mut i32;
            let options = arg3;

            let my_pid = {
                let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                percpu.scheduler.tasks[curr_idx].pid
            };

            // Scan all cores for a matching Zombie child; capture + tombstone it (state=Empty).
            let reap = |wpid: i64, parent: u64| -> Option<(u64, i64)> {
                unsafe {
                    let active = crate::smp::ACTIVE_CORES.load(core::sync::atomic::Ordering::SeqCst);
                    if let Some(cores) = &mut crate::percpu::PER_CPU {
                        for i in 0..active {
                            for task in cores[i].scheduler.tasks.iter_mut() {
                                if task.state == crate::scheduler::TaskState::Zombie
                                    && task.parent_pid == Some(parent)
                                    && (wpid == -1 || task.pid as i64 == wpid) {
                                    let out = (task.pid, task.exit_code);
                                    task.state = crate::scheduler::TaskState::Empty; // reaped tombstone
                                    return Some(out);
                                }
                            }
                        }
                    }
                }
                None
            };

            if let Some((cpid, code)) = reap(want_pid, my_pid) {
                if !wstatus.is_null() && is_valid_user_ptr(wstatus as *const u8, 4) {
                    unsafe { *wstatus = ((code as i32) & 0xff) << 8; } // WEXITSTATUS encoding
                }
                frame.rax = cpid;
            } else if options & WNOHANG != 0 {
                frame.rax = 0; // nothing ready, caller asked not to block
            } else {
                // Block until a child becomes reapable (re-poll; no explicit exit-waker exists yet).
                unsafe {
                    x86_64::instructions::interrupts::enable();
                    let mut result = 0u64;
                    loop {
                        if let Some((cpid, code)) = reap(want_pid, my_pid) {
                            if !wstatus.is_null() && is_valid_user_ptr(wstatus as *const u8, 4) {
                                *wstatus = ((code as i32) & 0xff) << 8;
                            }
                            result = cpid;
                            break;
                        }
                        let now = crate::time::UPTIME_MS.load(core::sync::atomic::Ordering::Relaxed);
                        let percpu = crate::percpu::current();
                        let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
                        percpu.scheduler.tasks[curr_idx].state = crate::scheduler::TaskState::Blocked;
                        percpu.scheduler.tasks[curr_idx].wake_tsc = now + 20; // re-scan every 20 ms
                        core::arch::asm!("int 0x41");
                        x86_64::instructions::hlt();
                    }
                    x86_64::instructions::interrupts::disable();
                    frame.rax = result;
                }
            }
        },
        
        // --- CUSTOM NYXOS SYSCALLS ---
        501 => {
            unsafe {
                let raw_color = arg5 as u32;

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

                        let _ = gpu.fill_rect(0x1400_0000, start_x, start_y, w, h, raw_color, pitch);
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
                        
                        let r = ((raw_color >> 16) & 0xFF) as u8;
                        let g = ((raw_color >> 8) & 0xFF) as u8;
                        let b = (raw_color & 0xFF) as u8;
                        let color = Color::new(r, g, b);
                        p.draw_rect(rect, color);
                    }
                }
            }
        },

        502 => { // sys_swap_buffers — present backbuffer -> owned scanout buffer (P1a), plane armed once.
             unsafe {
                 if let Some(gpu) = crate::drivers::gpu::intel::INTEL_GPU.lock().as_mut() {
                     if let Some(p) = &crate::gui::SCREEN_PAINTER {
                         let w = p.info.width as u32;
                         let h = p.info.height as u32;
                         let pitch = (p.info.stride * 4) as u32;
                         // BLT the backbuffer into our owned scanout buffer (active_gva == scanout_gva when
                         // P1a init succeeded, else the firmware-luck GVA 0). Same fast path as before —
                         // NO per-frame wait_for_idle or flip (those stalled on the fence timeout).
                         let _ = gpu.copy_rect(
                             0, 0, pitch, 0x1400_0000,
                             0, 0, pitch, gpu.active_gva,
                             w, h
                         );
                         gpu.submit_fence();
                         gpu.arm_scanout_plane(); // point the plane at scanout_gva once (no-op after first)
                     }
                 }
             }
        },

        538 => { // sys_swap_buffers_rect(x, y, w, h) — D1 region present.
            // Identical to 502 (backbuffer 0x1400_0000 -> active_gva at the full screen pitch) except it
            // BLTs only the damage sub-rect. Because src_x==dst_x==x, src_y==dst_y==y and the pitch is the
            // full screen stride for BOTH surfaces, the rect lands at the exact same scanout pixels 502
            // would have written — so it inherits whatever makes the full present visible (no new address
            // assumptions). The compositor uses this instead of 502 when only a partial region changed.
            let x = arg1 as u32;
            let y = arg2 as u32;
            let w = arg3 as u32;
            let h = arg4 as u32;
            unsafe {
                if let Some(gpu) = crate::drivers::gpu::intel::INTEL_GPU.lock().as_mut() {
                    if let Some(p) = &crate::gui::SCREEN_PAINTER {
                        let sw = p.info.width as u32;
                        let sh = p.info.height as u32;
                        let pitch = (p.info.stride * 4) as u32;
                        // Clamp the rect to the screen so a stale/oversized damage box can never blit
                        // out of bounds (the copy_rect BLT has no internal clip).
                        if w > 0 && h > 0 && x < sw && y < sh {
                            let cw = w.min(sw - x);
                            let ch = h.min(sh - y);
                            // D1 region present into the owned scanout buffer (active_gva == scanout_gva
                            // after P1a init). Single buffer, so a partial BLT is valid — the untouched
                            // pixels retain the previous frame. Arm the plane once (no-op after first).
                            let _ = gpu.copy_rect(
                                x, y, pitch, 0x1400_0000,
                                x, y, pitch, gpu.active_gva,
                                cw, ch
                            );
                            gpu.submit_fence();
                            gpu.arm_scanout_plane();
                        }
                    }
                }
            }
        },

        503 => { // sys_gpu_sync
             unsafe {
                 if let Some(gpu) = crate::drivers::gpu::intel::INTEL_GPU.lock().as_mut() {
                     gpu.submit_fence();
                     gpu.wait_for_idle();
                 }
             }
        },

        504 => {
            // Return true uptime, completely immune to CPU frequency scaling!
            frame.rax = crate::time::UPTIME_MS.load(core::sync::atomic::Ordering::Relaxed);
        },

        528 => {
            // SYS_GET_RTC: battery-backed wall clock, packed year<<40|month<<32|day<<24|hour<<16|min<<8|sec.
            frame.rax = crate::rtc::read_packed();
        },

        529 => {
            // SYS_CURSOR_INIT: bring up the display hardware cursor plane. 1 = enabled, 0 = fell back.
            frame.rax = crate::drivers::gpu::intel::cursor::init() as u64;
        },

        535 => {
            // SYS_CURSOR_SET_IMAGE(argb_ptr): upload a 64x64 ARGB cursor bitmap. 1 ok / 0 err.
            let ptr = arg1 as *const u32;
            let bytes = 64 * 64 * 4;
            if is_valid_user_ptr(ptr as *const u8, bytes) {
                let img = unsafe { core::slice::from_raw_parts(ptr, 64 * 64) };
                frame.rax = crate::drivers::gpu::intel::cursor::set_image(img) as u64;
            } else {
                frame.rax = EFAULT as u64;
            }
        },

        536 => {
            // SYS_GPU_COMPOSITE(list_ptr, count): GPU-composite `count` WindowQuads into the backbuffer.
            // 1 ok / 0 => caller falls back to CPU compositing.
            use crate::drivers::gpu::intel::render::compositor::WindowQuad;
            let ptr = arg1 as *const WindowQuad;
            let count = arg2 as usize;
            let bytes = count.saturating_mul(core::mem::size_of::<WindowQuad>());
            if count == 0 {
                frame.rax = 1;
            } else if is_valid_user_ptr(ptr as *const u8, bytes) {
                let quads = unsafe { core::slice::from_raw_parts(ptr, count) };
                frame.rax = crate::drivers::gpu::intel::render::compositor::composite(quads) as u64;
            } else {
                frame.rax = EFAULT as u64;
            }
        },

        537 => {
            // SYS_GPU_DRAW_TEXT(atlas_gva, atlas_w, atlas_h, atlas_pitch, glyph_ptr, glyph_count):
            // U5 — GPU-draw a batched glyph run sampling the userspace font atlas into the backbuffer.
            // 1 ok / 0 => caller falls back to CPU text.
            use crate::drivers::gpu::intel::render::text::GlyphQuad;
            let atlas_gva = arg1 as u32;
            let atlas_w = arg2 as u32;
            let atlas_h = arg3 as u32;
            let atlas_pitch = arg4 as u32;
            let ptr = arg5 as *const GlyphQuad;
            let count = arg6 as usize;
            let bytes = count.saturating_mul(core::mem::size_of::<GlyphQuad>());
            if count == 0 {
                frame.rax = 1;
            } else if is_valid_user_ptr(ptr as *const u8, bytes) {
                let glyphs = unsafe { core::slice::from_raw_parts(ptr, count) };
                frame.rax = crate::drivers::gpu::intel::render::text::draw_text(
                    atlas_gva, atlas_w, atlas_h, atlas_pitch, glyphs,
                ) as u64;
            } else {
                frame.rax = EFAULT as u64;
            }
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
                let mut mapped_phys = 0;
                let mut size = 0;
                
                if let Some(gpu) = crate::drivers::gpu::intel::INTEL_GPU.lock().as_ref() {
                    if gpu.backbuffer_phys != 0 {
                        mapped_phys = gpu.backbuffer_phys;
                        size = gpu.backbuffer_size;
                    }
                }
                
                if mapped_phys == 0 {
                    if let Some(p) = &mut crate::gui::SCREEN_PAINTER {
                        let virt_start = p.buffer.as_ptr() as u64;
                        if let Some(phys) = crate::memory::virt_to_phys(virt_start) {
                            mapped_phys = phys;
                            size = p.buffer.len() as u64;
                        }
                    }
                }
                
                if mapped_phys != 0 && size != 0 {
                    if let Ok(user_virt) = crate::memory::map_user_framebuffer(mapped_phys, size) {
                        frame.rax = user_virt;
                    } else { frame.rax = 0; }
                } else { frame.rax = 0; }
            }
        },
        509 => { // sys_gpu_map_shm
            let shm_id = arg1;
            let gva = arg2 as u32;
            let mut success = 0;
            
            // Map SHM pages into GGTT
            let registry = crate::memory::SHM_REGISTRY.lock();
            if let Some(block) = registry.iter().find(|b| b.id == shm_id) {
                if let Some(gpu) = crate::drivers::gpu::intel::INTEL_GPU.lock().as_mut() {
                    for (i, frame) in block.frames.iter().enumerate() {
                        unsafe {
                            gpu.map_ggtt_page(gva / 4096 + i as u32, frame.as_u64(), true);
                        }
                    }
                    success = 1;
                }
            }
            frame.rax = success;
        },

        512 => { // sys_gpu_copy_rect
            let src_gva = arg1 as u32;
            let dst_gva = arg2 as u32;
            let w = arg3 as u32;
            let h = arg4 as u32;
            let dst_x = arg5 as u32;
            let dst_y = arg6 as u32;
            
            unsafe {
                if let Some(gpu) = crate::drivers::gpu::intel::INTEL_GPU.lock().as_mut() {
                    if let Some(p) = &crate::gui::SCREEN_PAINTER {
                        let src_pitch = w * 4;
                        let dst_pitch = (p.info.stride * 4) as u32;
                        let _ = gpu.copy_rect(
                            0, 0, src_pitch, src_gva,
                            dst_x, dst_y, dst_pitch, dst_gva,
                            w, h
                        );
                    }
                }
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
        513 => { // sys_wait_vsync — returns 1 if the vblank was observed, 0 on timeout / no GPU.
            let mut seen = 0u64;
            unsafe {
                if let Some(gpu) = crate::drivers::gpu::intel::INTEL_GPU.lock().as_mut() {
                    seen = if gpu.wait_for_vsync() { 1 } else { 0 };
                }
            }
            frame.rax = seen;
        },

        // ---------------------------------------------------------------------
        // Phase 9: mini-GL 3D syscalls. These wrap the RCS render engine's Scene
        // path (SSAA-antialiased, textured, multi-mesh) for userspace. The heavy
        // data (verts/texels) is copied into kernel Vecs once at upload; per frame
        // only the MVP matrices cross the boundary. See render/gl.rs.
        // ---------------------------------------------------------------------
        514 => { // sys_gl_init(width, height, dst_pixels_ptr) -> 1 ok / 0 err
            // Windowed GL: the app passes the CPU pointer to ITS OWN SHM window pixel buffer. The GL
            // path resolves into a private kernel backbuffer and copies it here each frame, so the
            // compositor composites glcube as a normal window (no scanout blit). Validate the buffer.
            let width = arg1 as u32;
            let height = arg2 as u32;
            let dst_ptr = arg3 as *const u8;
            let bytes = (width as usize).saturating_mul(height as usize).saturating_mul(4);
            if width == 0 || height == 0 || bytes == 0 || !is_valid_user_ptr(dst_ptr, bytes) {
                frame.rax = 0;
            } else {
                match crate::drivers::gpu::intel::render::gl::gl_init(width, height, dst_ptr as u64) {
                    Ok(()) => frame.rax = 1,
                    Err(_) => frame.rax = 0,
                }
            }
        },

        515 => { // sys_gl_upload_mesh(desc_ptr) -> handle (>=0) or u64::MAX on error
            // desc layout (repr(C), all u64): [verts_ptr, vert_count, idx_ptr, idx_count,
            //                                  texel_ptr, tex_w, tex_h]
            let desc_ptr = arg1 as *const u64;
            if !is_valid_user_ptr(desc_ptr as *const u8, 7 * 8) {
                frame.rax = u64::MAX;
            } else {
                let d = unsafe { core::slice::from_raw_parts(desc_ptr, 7) };
                let verts_ptr = d[0] as *const crate::drivers::gpu::intel::render::gl::GlVertex;
                let vert_count = d[1] as usize;
                let idx_ptr = d[2] as *const u32;
                let idx_count = d[3] as usize;
                let texel_ptr = d[4] as *const u32;
                let tex_w = d[5] as u32;
                let tex_h = d[6] as u32;
                let vbytes = vert_count.saturating_mul(28); // 7 f32
                let ibytes = idx_count.saturating_mul(4);
                let tbytes = (tex_w as usize).saturating_mul(tex_h as usize).saturating_mul(4);
                if vert_count == 0 || idx_count == 0 || tex_w == 0 || tex_h == 0
                    || !is_valid_user_ptr(verts_ptr as *const u8, vbytes)
                    || !is_valid_user_ptr(idx_ptr as *const u8, ibytes)
                    || !is_valid_user_ptr(texel_ptr as *const u8, tbytes)
                {
                    frame.rax = u64::MAX;
                } else {
                    let verts = unsafe { core::slice::from_raw_parts(verts_ptr, vert_count) };
                    let indices = unsafe { core::slice::from_raw_parts(idx_ptr, idx_count) };
                    let texels = unsafe {
                        core::slice::from_raw_parts(texel_ptr, (tex_w * tex_h) as usize)
                    };
                    match crate::drivers::gpu::intel::render::gl::gl_upload_mesh(
                        verts, indices, texels, tex_w, tex_h,
                    ) {
                        Ok(h) => frame.rax = h as u64,
                        Err(_) => frame.rax = u64::MAX,
                    }
                }
            }
        },

        516 => { // sys_gl_render(mvps_ptr, count) -> 1 ok / 0 err. mvps = count * 16 f32 (column-major).
            let mvps_ptr = arg1 as *const f32;
            let count = arg2 as usize;
            let floats = count.saturating_mul(16);
            let bytes = floats.saturating_mul(4);
            if count == 0 || !is_valid_user_ptr(mvps_ptr as *const u8, bytes) {
                frame.rax = 0;
            } else {
                let flat = unsafe { core::slice::from_raw_parts(mvps_ptr, floats) };
                match crate::drivers::gpu::intel::render::gl::gl_render_flat(flat, count) {
                    Ok(()) => frame.rax = 1,
                    Err(_) => frame.rax = 0,
                }
            }
        },

        527 => { // sys_gl_reset()
            crate::drivers::gpu::intel::render::gl::gl_reset();
            frame.rax = 1;
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
                // Copy as much of the boot log as the caller's buffer holds. (Was
                // capped at 8192, which truncated the GPU pipeline decode mid-stream
                // in the sysmon viewer.) BOOT_LOG_IDX is always <= BOOT_LOG_SIZE.
                let log_len = crate::serial::BOOT_LOG_IDX;
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

        539 => { // SYS_DESTROY_SHM(id, base_vaddr) — R1: owner frees a resized-away buffer.
            let id = arg1;
            let base_vaddr = arg2;
            frame.rax = if crate::memory::destroy_shm_block(id, base_vaddr) { 1 } else { 0 };
        },

        540 => { // SYS_UNMAP_SHM(base_vaddr, size) — R1: release caller's mapping only (no frame free).
            let base_vaddr = arg1;
            let size = arg2 as usize;
            crate::memory::unmap_shm_range(base_vaddr, size);
            frame.rax = 1;
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
        534 => { 
            frame.rax = sys_dns_resolve(arg1 as usize, arg2 as usize); 
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
                            let is_non_blocking = sock.non_blocking;
                            drop(sock); // Drop lock during the yield cycle

                            loop {
                                crate::drivers::net::poll_network();
                                let mut sockets_lock = crate::drivers::net::GLOBAL_SOCKETS.lock();
                                if let Some(sockets) = sockets_lock.as_mut() {
                                    let socket = sockets.get_mut::<smoltcp::socket::tcp::Socket>(handle);
                                    if socket.can_recv() {
                                        let user_slice = unsafe { core::slice::from_raw_parts_mut(buf_ptr, len) };
                                        if let Ok(received) = socket.recv_slice(user_slice) {
                                            if received > 0 { return received as isize; }
                                        }
                                    } else if !socket.may_recv() {
                                        return 0; // Connection closed gracefully (EOF)
                                    }
                                }
                                drop(sockets_lock);

                                if is_non_blocking {
                                    return EAGAIN as isize; // Async bypass
                                }

                                // Yield thread safely while waiting for new hardware frames
                                unsafe {
                                    x86_64::instructions::interrupts::enable();
                                    core::arch::asm!("int 0x41");
                                    x86_64::instructions::interrupts::disable();
                                }
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
                            let is_non_blocking = sock.non_blocking;
                            drop(sock);

                            loop {
                                crate::drivers::net::poll_network();
                                let mut sockets_lock = crate::drivers::net::GLOBAL_SOCKETS.lock();
                                if let Some(sockets) = sockets_lock.as_mut() {
                                    let socket = sockets.get_mut::<smoltcp::socket::tcp::Socket>(handle);
                                    if socket.can_send() {
                                        if let Ok(sent) = socket.send_slice(buf_slice) {
                                            return sent as isize;
                                        }
                                    }
                                }
                                drop(sockets_lock);

                                if is_non_blocking {
                                    return EAGAIN as isize;
                                }

                                unsafe {
                                    x86_64::instructions::interrupts::enable();
                                    core::arch::asm!("int 0x41");
                                    x86_64::instructions::interrupts::disable();
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

// Add these POSIX error code constants at the top of interrupts.rs if they aren't there:
const EINPROGRESS: i64 = -115;
const ECONNREFUSED: i64 = -111;
const ETIMEDOUT: i64 = -110;

#[no_mangle]
pub extern "C" fn sys_socket(_domain: u64, _typ: u64, _protocol: u64) -> i64 {
    let mut sockets_lock = crate::drivers::net::GLOBAL_SOCKETS.lock();
    if sockets_lock.is_none() { *sockets_lock = Some(smoltcp::iface::SocketSet::new(alloc::vec![])); }
    
    if let Some(sockets) = sockets_lock.as_mut() {
        // 🔥 MILESTONE 3.1: Linux checks if SOCK_NONBLOCK (2048 / 0x800) is set in the type parameter
        let is_non_blocking = (_typ & 2048) != 0;
        let clean_type = _typ & !2048; // Strip the flag out to get the raw type (1 = TCP, 2 = UDP)

        let local_port = NEXT_LOCAL_PORT.fetch_add(1, core::sync::atomic::Ordering::Relaxed);
        
        let handle = if clean_type == 1 {
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

        let kind = if clean_type == 1 { SocketKind::Tcp(handle) } else { SocketKind::Udp(handle) };
        let ks = KernelSocket { kind, local_port, remote: None, non_blocking: is_non_blocking };
        
        if KERNEL_CR3.load(Ordering::Relaxed) == 0 { return EBADF; }
        let percpu = crate::percpu::current();
        
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

#[no_mangle]
pub extern "C" fn sys_connect(fd: usize, addr_ptr: *const u8, addr_len: usize) -> i64 {
    if addr_len < 16 || !is_valid_user_ptr(addr_ptr, addr_len) { return EFAULT; }
    if KERNEL_CR3.load(Ordering::Relaxed) == 0 { return EBADF; }
    
    let percpu = crate::percpu::current();
    let curr_idx = percpu.scheduler.core_task_idx[percpu.logical_id as usize % 32];
    if curr_idx >= percpu.scheduler.tasks.len() { return EBADF; }
    
    let sockaddr = unsafe { &*(addr_ptr as *const SockAddrIn) };
    if sockaddr.sin_family != 2 { return EINVAL; }

    let port = u16::from_be(sockaddr.sin_port);
    let ip = sockaddr.sin_addr;
    let task = &mut percpu.scheduler.tasks[curr_idx];
    
    if fd >= 32 { return EBADF; }
    
    if let Some(FileDescriptor::Socket(sock_mtx)) = &task.fd_table[fd] {
        let mut sock = sock_mtx.lock();
        let addr = IpAddress::Ipv4(Ipv4Address::new(ip[0], ip[1], ip[2], ip[3]));
        sock.remote = Some(IpEndpoint::new(addr, port));
        
        if let SocketKind::Tcp(handle) = sock.kind {
            let local_port = sock.local_port;
            let is_non_blocking = sock.non_blocking;
            
            {
                let mut sockets_lock = crate::drivers::net::GLOBAL_SOCKETS.lock();
                let mut iface_lock = crate::drivers::net::NET_IFACE.lock();
                if let (Some(sockets), Some(iface)) = (sockets_lock.as_mut(), iface_lock.as_mut()) {
                    let socket = sockets.get_mut::<smoltcp::socket::tcp::Socket>(handle);
                    if socket.connect(iface.context(), IpEndpoint::new(addr, port), local_port).is_err() {
                        return ECONNREFUSED;
                    }
                } else { return EBADF; }
            }

            // Drop lock while we poll/sleep to avoid cross-thread deadlocks
            drop(sock);

            // 🔥 MILESTONE 3.1 & 3.3: Handling Blocking vs Non-Blocking states + State Transitions
            if is_non_blocking {
                return EINPROGRESS; // Return instantly for async GUI apps
            }

            // Blocking Loop: Put thread to sleep until TCP handshakes finish or fail
            let start_ms = crate::time::UPTIME_MS.load(Ordering::Relaxed);
            loop {
                crate::drivers::net::poll_network();
                
                let mut sockets_lock = crate::drivers::net::GLOBAL_SOCKETS.lock();
                if let Some(sockets) = sockets_lock.as_mut() {
                    let socket = sockets.get_mut::<smoltcp::socket::tcp::Socket>(handle);
                    
                    if socket.state() == smoltcp::socket::tcp::State::Established {
                        return 0; // Connected!
                    }
                    if socket.state() == smoltcp::socket::tcp::State::Closed {
                        return ECONNREFUSED; // Connection rejected
                    }
                }
                drop(sockets_lock);

                // Timeout check (safely timeout after 10 seconds)
                let current_ms = crate::time::UPTIME_MS.load(Ordering::Relaxed);
                if current_ms.saturating_sub(start_ms) > 10000 {
                    return ETIMEDOUT;
                }

                // Yield the CPU core to other threads while waiting for the network card interrupt
                unsafe {
                    x86_64::instructions::interrupts::enable();
                    core::arch::asm!("int 0x41");
                    x86_64::instructions::interrupts::disable();
                }
            }
        }
        return 0;
    }
    EBADF
}

pub extern "x86-interrupt" fn rtl8168_interrupt_handler(_stack_frame: x86_64::structures::idt::InterruptStackFrame) {
   
    crate::serial_println!("[ISR] Hardware Interrupt Fired! NIC Woke up the CPU!");
    
    crate::drivers::net::NETWORK_PENDING.store(true, core::sync::atomic::Ordering::Release);
    crate::apic::end_of_interrupt();
}

// Domain Name Resolution (DNS)
#[no_mangle]
pub extern "C" fn sys_dns_resolve(hostname_ptr: usize, hostname_len: usize) -> u64 {
    if hostname_len == 0 { return 0; }
    if KERNEL_CR3.load(Ordering::Relaxed) == 0 { return 0; }
    
    let hostname_slice = unsafe { core::slice::from_raw_parts(hostname_ptr as *const u8, hostname_len) };
    let hostname_str = match core::str::from_utf8(hostname_slice) {
        Ok(s) => s,
        Err(_) => return 0,
    };

    crate::serial_println!("[DNS] Resolving: {}", hostname_str);

    // 1. Fire the DNS Query
    let query_handle = {
        let mut sockets_lock = crate::drivers::net::GLOBAL_SOCKETS.lock();
        let mut iface_lock = crate::drivers::net::NET_IFACE.lock();
        let dns_lock = crate::drivers::net::DNS_HANDLE.lock();
        
        if let (Some(sockets), Some(iface), Some(dns_handle)) = (sockets_lock.as_mut(), iface_lock.as_mut(), *dns_lock) {
            let dns_socket = sockets.get_mut::<smoltcp::socket::dns::Socket>(dns_handle);
            match dns_socket.start_query(iface.context(), hostname_str, smoltcp::wire::DnsQueryType::A) {
                Ok(handle) => handle,
                Err(e) => {
                    crate::serial_println!("[DNS] Failed to start query: {:?}", e);
                    return 0;
                }
            }
        } else {
            return 0;
        }
    };

    let start_ms = crate::time::UPTIME_MS.load(Ordering::Relaxed);
    
    // 2. Safely Block until the DNS Server Replies
    loop {
        crate::drivers::net::poll_network();
        
        let mut sockets_lock = crate::drivers::net::GLOBAL_SOCKETS.lock();
        if let Some(sockets) = sockets_lock.as_mut() {
            let dns_handle = crate::drivers::net::DNS_HANDLE.lock().unwrap();
            let dns_socket = sockets.get_mut::<smoltcp::socket::dns::Socket>(dns_handle);
            
            match dns_socket.get_query_result(query_handle) {
                Ok(addrs) => {
                    for addr in addrs.iter() {
                        if let smoltcp::wire::IpAddress::Ipv4(ipv4) = addr {
                            crate::serial_println!("[DNS] Resolved {} -> {}", hostname_str, ipv4);
                            let octets = ipv4.0;
                            // Pack the 4 bytes into a single u64 to return across the Syscall boundary
                            return (octets[0] as u64) | ((octets[1] as u64) << 8) | ((octets[2] as u64) << 16) | ((octets[3] as u64) << 24);
                        }
                    }
                    return 0; // No IPv4 found
                },
                Err(smoltcp::socket::dns::GetQueryResultError::Pending) => {
                    // Still waiting...
                },
                Err(_) => {
                    crate::serial_println!("[DNS] Query Failed/NXDOMAIN.");
                    return 0; 
                }
            }
        }
        drop(sockets_lock);

        let current_ms = crate::time::UPTIME_MS.load(Ordering::Relaxed);
        if current_ms.saturating_sub(start_ms) > 5000 {
            crate::serial_println!("[DNS] Timeout.");
            return 0;
        }

        // Sleep the thread via the Scheduler Gateway
        unsafe {
            x86_64::instructions::interrupts::enable();
            core::arch::asm!("int 0x41");
            x86_64::instructions::interrupts::disable();
        }
    }
}