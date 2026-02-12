use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use lazy_static::lazy_static;
use pic8259::ChainedPics;
use spin::Mutex;
use crate::gdt;
use core::arch::naked_asm;
use crate::scheduler::SCHEDULER;
use crate::gui::{Painter, Color}; 
use alloc::format;
use crate::fs; 

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static PICS: Mutex<ChainedPics> = Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

// --- 1. TIMER INTERRUPT ---
#[unsafe(naked)]
extern "x86-interrupt" fn timer_interrupt_handler(_sf: InterruptStackFrame) {
    naked_asm!(
        "push rax", "push rbx", "push rcx", "push rdx", "push rsi", "push rdi",
        "push r8", "push r9", "push r10", "push r11", "push r12", "push r13", "push r14", "push r15", "push rbp",
        "mov rdi, rsp", 
        "call task_switch_wrapper",
        "mov rsp, rax",
        "pop rbp", "pop r15", "pop r14", "pop r13", "pop r12", "pop r11", "pop r10", "pop r9",
        "pop r8", "pop rdi", "pop rsi", "pop rdx", "pop rcx", "pop rbx", "pop rax",
        "mov al, 0x20", "out 0x20, al",
        "iretq"
    );
}

#[no_mangle]
pub extern "C" fn task_switch_wrapper(rsp: u64) -> u64 {
    unsafe { 
        crate::time::tick();
        crate::shell::process_keys(); 
        if let Some(s) = &mut SCHEDULER { 
            return s.schedule(rsp); 
        } 
        rsp 
    }
}

// --- 2. SYSCALL HANDLERS ---
#[unsafe(naked)]
pub extern "C" fn syscall_asm_wrapper() {
    naked_asm!(
        "swapgs", 
        "mov gs:[8], rsp",      
        "mov rsp, gs:[0]",      
        "push {user_data_sel}", "push gs:[8]", "push r11", "push {user_code_sel}", "push rcx",
        "push rax", "push rdi", "push rsi", "push rdx", "push r10", "push r8", "push r9",
        "mov rdi, rsp", 
        "call syscall_rust_dispatcher",
        "pop r9", "pop r8", "pop r10", "pop rdx", "pop rsi", "pop rdi", "pop rax",
        "swapgs", 
        "iretq",
        user_data_sel = const (0x18 | 3), 
        user_code_sel = const (0x20 | 3),
    );
}

#[repr(C)] 
struct SyscallRegisters { 
    r9: u64, r8: u64, r10: u64, rdx: u64, rsi: u64, rdi: u64, rax: u64 
}

#[no_mangle]
extern "C" fn syscall_rust_dispatcher(ptr: *mut SyscallRegisters) {
    let regs = unsafe { &mut *ptr };
    match regs.rax {
        0 => loop { x86_64::instructions::hlt() }, // sys_exit
        1 => { // sys_print
            let c = regs.rdi as u8 as char; 
            crate::window::WINDOW_MANAGER.lock().console_print(c); 
        },
        2 => { // sys_read_key
            if let Some(c) = crate::shell::pop_char() { 
                regs.rax = c as u64; 
            } else { 
                regs.rax = 0; 
            } 
        },
        3 => { // sys_get_mouse
             let m = crate::mouse::MOUSE_STATE.lock();
             let l = if m.left_click { 1u64 } else { 0 };
             let r = if m.right_click { 1u64 } else { 0 };
             regs.rax = (l << 63) | (r << 62) | ((m.x as u64 & 0xFFFF) << 32) | (m.y as u64 & 0xFFFF);
        },
        4 => { // sys_draw_pixel
            crate::window::WINDOW_MANAGER.lock().put_desktop_pixel(regs.rdi as usize, regs.rsi as usize, regs.rdx as u32); 
        },
        6 => { // sys_get_screen_info
             if let Some(p) = unsafe { &crate::SCREEN_PAINTER } {
                 regs.rax = ((p.info.width as u64) << 32) | (p.info.height as u64);
                 regs.rdi = p.info.stride as u64;
                 regs.rsi = p.info.bytes_per_pixel as u64; 
             }
        },
        7 => { // sys_map_framebuffer
             let phys = unsafe { crate::gui::FRAMEBUFFER_PHYS_ADDR };
             if let Some(p) = unsafe { &crate::SCREEN_PAINTER } {
                 let size = (p.info.stride * p.info.height * 4) as u64 + 4096;
                 if let Ok(virt) = crate::memory::map_user_framebuffer(phys, size) { 
                     regs.rax = virt; 
                 } else { 
                     regs.rax = 0; 
                 }
             }
        },
        8 => { // sys_get_time
            regs.rax = crate::time::get_ticks(); 
        },
        9 => { // sys_alloc
             let size = regs.rdi; 
             match crate::memory::map_user_memory(size) {
                 Ok(addr) => regs.rax = addr,
                 Err(_) => regs.rax = 0, 
             }
        },
        10 => { // SYS_FS_COUNT
            regs.rax = fs::FS.lock().files.len() as u64;
        },
        11 => { // SYS_FS_GET_NAME
            let idx = regs.rdi as usize;
            let buf_ptr = regs.rsi as *mut u8;
            let max_len = regs.rdx as usize;
            let fs = fs::FS.lock();
            if idx < fs.files.len() {
                let name = &fs.files[idx].name;
                let len = core::cmp::min(name.len(), max_len);
                unsafe { core::ptr::copy(name.as_ptr(), buf_ptr, len); }
                regs.rax = len as u64;
            } else { regs.rax = 0; }
        },
        12 => { // SYS_FS_OPEN
            let ptr = regs.rdi as *const u8;
            let len = regs.rsi as usize;
            let slice = unsafe { core::slice::from_raw_parts(ptr, len) };
            if let Ok(name) = core::str::from_utf8(slice) {
                if let Some(file) = fs::FS.lock().get_file_by_name(name) {
                    regs.rax = file.id as u64;
                } else { regs.rax = 0; }
            }
        },
        13 => { // SYS_FS_READ
            let id = regs.rdi as usize;
            let buf_ptr = regs.rsi as *mut u8;
            let max_len = regs.rdx as usize;
            let fs = fs::FS.lock();
            if let Some(file) = fs.get_file_by_id(id) {
                let len = core::cmp::min(file.data.len(), max_len);
                unsafe { core::ptr::copy(file.data.as_ptr(), buf_ptr, len); }
                regs.rax = len as u64;
            } else { regs.rax = 0; }
        },
        14 => { // SYS_FS_WRITE
            let name_ptr = regs.rdi as *const u8;
            let name_len = regs.rsi as usize;
            let data_ptr = regs.rdx as *const u8;
            let data_len = regs.r10 as usize; 
            
            let name_slice = unsafe { core::slice::from_raw_parts(name_ptr, name_len) };
            let data_slice = unsafe { core::slice::from_raw_parts(data_ptr, data_len) };
            
            if let Ok(name) = core::str::from_utf8(name_slice) {
                 regs.rax = fs::FS.lock().create_file(name, data_slice) as u64;
            } else { regs.rax = 0; }
        },
        _ => {}
    }
}

// --- 3. IDT SETUP ---
lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(bp_handler);
        unsafe { 
            idt.double_fault.set_handler_fn(df_handler).set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX); 
        }
        idt.page_fault.set_handler_fn(pf_handler);
        idt[PIC_1_OFFSET as usize].set_handler_fn(timer_interrupt_handler);
        idt[(PIC_1_OFFSET + 1) as usize].set_handler_fn(kb_handler); 
        idt[(PIC_2_OFFSET + 4) as usize].set_handler_fn(mouse_handler); 
        idt
    };
}

pub fn init_idt() { 
    IDT.load(); 
    unsafe { 
        PICS.lock().initialize(); 
        PICS.lock().write_masks(0xF8, 0xEF); 
    } 
}

pub fn init_syscalls() {
    use x86_64::registers::model_specific::{Efer, EferFlags, LStar, Star, SFMask, KernelGsBase};
    use x86_64::registers::rflags::RFlags;
    use x86_64::structures::gdt::SegmentSelector;
    use x86_64::VirtAddr;
    
    unsafe {
        static mut SYSCALL_STACK: [u8; 4096 * 5] = [0; 4096 * 5];
        let stack_top = VirtAddr::from_ptr(&SYSCALL_STACK).as_u64() + (4096 * 5);
        crate::interrupts::GS_DATA.kernel_stack = stack_top;
        
        KernelGsBase::write(VirtAddr::new(&crate::interrupts::GS_DATA as *const _ as u64));
        Efer::update(|flags| { flags.insert(EferFlags::SYSTEM_CALL_EXTENSIONS); });
        LStar::write(VirtAddr::new(syscall_asm_wrapper as *const () as u64));
        
        let kernel_code = gdt::get_kernel_code_selector();
        let kernel_data = gdt::get_kernel_data_selector();
        let user_code = gdt::get_user_code_selector();
        let user_data = gdt::get_user_data_selector();
        
        Star::write(
            SegmentSelector(user_code), 
            SegmentSelector(user_data), 
            SegmentSelector(kernel_code), 
            SegmentSelector(kernel_data)
        ).unwrap();
        
        SFMask::write(RFlags::INTERRUPT_FLAG | RFlags::TRAP_FLAG);
    }
}

#[repr(C)] 
pub struct KernelGsData { pub kernel_stack: u64, pub user_stack: u64 }
static mut GS_DATA: KernelGsData = KernelGsData { kernel_stack: 0, user_stack: 0 };

extern "x86-interrupt" fn bp_handler(_: InterruptStackFrame) {}

extern "x86-interrupt" fn df_handler(_: InterruptStackFrame, _: u64) -> ! { loop {} }

extern "x86-interrupt" fn pf_handler(sf: InterruptStackFrame, ec: PageFaultErrorCode) {
    let fault_addr: u64;
    unsafe { core::arch::asm!("mov {}, cr2", out(reg) fault_addr); }
    unsafe {
        if let Some(p) = &mut crate::SCREEN_PAINTER {
            p.clear(crate::gui::Color::RED);
            use alloc::format;
            let msg = format!("PAGE FAULT at 0x{:x}", fault_addr);
            p.draw_string(20, 20, &msg, crate::gui::Color::WHITE);
        }
    }
    loop {}
}

extern "x86-interrupt" fn kb_handler(_: InterruptStackFrame) {
    use x86_64::instructions::port::Port;
    let scancode: u8 = unsafe { Port::<u8>::new(0x60).read() };
    if let Some(c) = scancode_to_char(scancode) { crate::shell::handle_char(c); }
    crate::shell::handle_key(scancode);
    unsafe { PICS.lock().notify_end_of_interrupt(PIC_1_OFFSET + 1); }
}

extern "x86-interrupt" fn mouse_handler(_: InterruptStackFrame) {
    use x86_64::instructions::port::Port;
    let b = unsafe { Port::<u8>::new(0x60).read() };
    crate::mouse::handle_interrupt(b);
    unsafe { PICS.lock().notify_end_of_interrupt(PIC_2_OFFSET + 4); }
}

// --- KEYBOARD MAPPING FIXED ---
fn scancode_to_char(scancode: u8) -> Option<char> {
    match scancode {
        0x02..=0x0B => Some(match scancode { 
            0x02 => '1', 0x03 => '2', 0x04 => '3', 0x05 => '4', 0x06 => '5',
            0x07 => '6', 0x08 => '7', 0x09 => '8', 0x0A => '9', 0x0B => '0', _=>'0'
        }),
        0x10..=0x19 => Some(match scancode { 
            0x10 => 'q', 0x11 => 'w', 0x12 => 'e', 0x13 => 'r', 0x14 => 't',
            0x15 => 'y', 0x16 => 'u', 0x17 => 'i', 0x18 => 'o', 0x19 => 'p', _=>' '
        }),
        0x1E..=0x26 => Some(match scancode { 
            0x1E => 'a', 0x1F => 's', 0x20 => 'd', 0x21 => 'f', 0x22 => 'g',
            0x23 => 'h', 0x24 => 'j', 0x25 => 'k', 0x26 => 'l', _=>' '
        }),
        0x2C..=0x32 => Some(match scancode { 
            0x2C => 'z', 0x2D => 'x', 0x2E => 'c', 0x2F => 'v', 0x30 => 'b',
            0x31 => 'n', 0x32 => 'm', _=>' '
        }),
        0x39 => Some(' '), 
        0x1C => Some('\n'), 
        0x0E => Some('\x08'), 
        // --- ADDED MAPPINGS ---
        0x33 => Some(','),
        0x34 => Some('.'), // The Fix for "cat readme.txt"
        0x35 => Some('/'),
        0x0C => Some('-'),
        _ => None,
    }
}