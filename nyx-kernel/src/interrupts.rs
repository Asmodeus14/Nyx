use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use lazy_static::lazy_static;
use pic8259::ChainedPics;
use spin::Mutex;
use crate::gdt;
use pc_keyboard::{layouts, DecodedKey, HandleControl, Keyboard, ScancodeSet1}; 
use crate::gui::Painter; 
use x86_64::VirtAddr;
use core::arch::naked_asm;

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

static mut SYSCALL_STACK: [u8; 4096 * 5] = [0; 4096 * 5];

#[repr(C)]
pub struct KernelGsData {
    pub kernel_stack: u64, 
    pub user_stack: u64,   
}

static mut GS_DATA: KernelGsData = KernelGsData { kernel_stack: 0, user_stack: 0 };

// --- SYSCALL ENTRY POINT (ASM WRAPPER) ---
#[unsafe(naked)]
pub extern "C" fn syscall_asm_wrapper() {
    unsafe {
        naked_asm!(
            "swapgs",
            "mov gs:[8], rsp",      // Save User Stack
            "mov rsp, gs:[0]",      // Load Kernel Stack
            
            // Push Context
            "push {user_data_sel}", 
            "push gs:[8]",          
            "push r11",             // RFLAGS
            "push {user_code_sel}", 
            "push rcx",             // RIP
            
            // Push Registers (C Calling Convention)
            "push rax",
            "push rdi",
            "push rsi",
            "push rdx",
            "push rcx",             
            "push r8",
            "push r9",
            
            "mov rdi, rsp",
            "call syscall_rust_dispatcher",
            
            // Restore Context
            "pop r9",
            "pop r8",
            "pop rcx",
            "pop rdx",
            "pop rsi",
            "pop rdi",
            "pop rax",
            
            "swapgs",
            "iretq",
            user_data_sel = const (0x18 | 3), 
            user_code_sel = const (0x20 | 3),
        );
    }
}

#[repr(C)]
struct SyscallRegisters {
    pub r9: u64, 
    pub r8: u64, 
    pub rcx: u64, 
    pub rdx: u64,
    pub rsi: u64, 
    pub rdi: u64, 
    pub rax: u64,
}

// --- SYSCALL DISPATCHER ---
#[no_mangle]
extern "C" fn syscall_rust_dispatcher(stack_ptr: *mut SyscallRegisters) {
    let regs = unsafe { &*stack_ptr };
    match regs.rax {
        0 => { // EXIT
            use crate::gui::Color;
            if let Some(painter) = unsafe { &mut crate::SCREEN_PAINTER } {
                painter.draw_string(20, 500, "PROCESS FINISHED.", Color::RED);
            }
            loop { x86_64::instructions::hlt(); }
        },
        1 => { // PRINT
            let char_to_print = regs.rdi as u8 as char;
            crate::window::WINDOW_MANAGER.lock().console_print(char_to_print);
        },
        2 => { // READ_KEY
             if let Some(c) = crate::shell::pop_char() {
                 unsafe { (*stack_ptr).rax = c as u64; }
             } else {
                 unsafe { (*stack_ptr).rax = 0; }
             }
        },
        3 => { // GET_MOUSE
            let mouse = crate::mouse::MOUSE_STATE.lock();
            let x = (mouse.x as u64) & 0xFFFF;
            let y = (mouse.y as u64) & 0xFFFF;
            let left = if mouse.left_click { 1u64 } else { 0u64 };
            let right = if mouse.right_click { 1u64 } else { 0u64 };
            let packed = (left << 63) | (right << 62) | (x << 32) | y;
            unsafe { (*stack_ptr).rax = packed; }
        },
        4 => { // SYS_DRAW
             x86_64::instructions::interrupts::without_interrupts(|| {
                crate::window::WINDOW_MANAGER.lock().put_desktop_pixel(regs.rdi as usize, regs.rsi as usize, regs.rdx as u32);
             });
        },
        5 => { // SYS_BLIT
            let x = regs.rdi as usize;
            let y = regs.rsi as usize;
            let w = regs.rdx as usize;
            let h = regs.r8 as usize;
            let ptr = regs.r9 as *const u32;

            if w < 3000 && h < 3000 {
                let buffer_len = w * h;
                unsafe {
                    if !ptr.is_null() {
                        let buffer = core::slice::from_raw_parts(ptr, buffer_len);
                        x86_64::instructions::interrupts::without_interrupts(|| {
                             crate::window::WINDOW_MANAGER.lock().blit_desktop_rect(x, y, w, h, buffer);
                        });
                    }
                }
            }
        },
        6 => { // SYS_GET_SCREEN_INFO
             x86_64::instructions::interrupts::without_interrupts(|| {
                let wm = crate::window::WINDOW_MANAGER.lock();
                let w = wm.screen_width as u64;
                let h = wm.screen_height as u64;
                let packed = (w << 32) | h;
                unsafe { (*stack_ptr).rax = packed; }
             });
        },
        7 => { // SYS_MAP_FRAMEBUFFER
             x86_64::instructions::interrupts::without_interrupts(|| {
                let phys = unsafe { crate::gui::FRAMEBUFFER_PHYS_ADDR };
                if phys != 0 {
                    let wm = crate::window::WINDOW_MANAGER.lock();
                    let size = (wm.screen_width * wm.screen_height * 4) as u64;
                    drop(wm);
                    
                    match crate::memory::map_user_framebuffer(phys, size) {
                        Ok(virt_addr) => unsafe { (*stack_ptr).rax = virt_addr; },
                        Err(_) => unsafe { (*stack_ptr).rax = 0; }
                    }
                } else {
                    unsafe { (*stack_ptr).rax = 0; }
                }
             });
        },
        8 => { // SYS_GET_TIME (For 60FPS lock)
             unsafe { (*stack_ptr).rax = crate::time::get_ticks(); }
        },
        _ => {}
    }
}

lazy_static! {
    static ref IDT: InterruptDescriptorTable = {
        let mut idt = InterruptDescriptorTable::new();
        idt.breakpoint.set_handler_fn(breakpoint_handler);
        unsafe {
            idt.double_fault.set_handler_fn(double_fault_handler)
                .set_stack_index(gdt::DOUBLE_FAULT_IST_INDEX);
        }
        idt.page_fault.set_handler_fn(page_fault_handler);
        idt[PIC_1_OFFSET as usize].set_handler_fn(timer_interrupt_handler);
        idt[(PIC_1_OFFSET + 1) as usize].set_handler_fn(keyboard_interrupt_handler);
        idt[(PIC_2_OFFSET + 4) as usize].set_handler_fn(mouse_interrupt_handler);
        idt
    };
}

pub fn init_idt() {
    IDT.load();
    unsafe {
        let mut pics = PICS.lock();
        pics.initialize();
        pics.write_masks(0xF8, 0xEF);
    }
}

pub fn init_syscalls() {
    use x86_64::registers::model_specific::{Efer, EferFlags, LStar, Star, SFMask, KernelGsBase};
    use x86_64::registers::rflags::RFlags;
    use x86_64::structures::gdt::SegmentSelector;

    unsafe {
        let stack_top = VirtAddr::from_ptr(&SYSCALL_STACK).as_u64() + (4096 * 5);
        GS_DATA.kernel_stack = stack_top;
        KernelGsBase::write(VirtAddr::new(&GS_DATA as *const _ as u64));

        Efer::update(|flags| {
            flags.insert(EferFlags::SYSTEM_CALL_EXTENSIONS);
        });

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

// --- TIMER INTERRUPT (TURBO MODE) ---
extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    crate::time::tick();
    
    // TURBO MODE: Poll USB on every tick (1ms) for max responsiveness
    if let Some(mut usb_lock) = crate::usb::USB_CONTROLLER.try_lock() {
        if let Some(controller) = usb_lock.as_mut() {
            // Poll all 4 possible ports
            for i in 1..=4 { controller.poll_mouse(i); }
        }
    }

    crate::mouse::drain_queue();
    crate::shell::process_keys();

    unsafe { PICS.lock().notify_end_of_interrupt(PIC_1_OFFSET); }
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;
    lazy_static! {
        static ref KEYBOARD: Mutex<Keyboard<layouts::Us104Key, ScancodeSet1>> =
            Mutex::new(Keyboard::new(ScancodeSet1::new(), layouts::Us104Key, HandleControl::Ignore));
    }
    let mut keyboard = KEYBOARD.lock();
    let mut port = Port::new(0x60);
    let scancode: u8 = unsafe { port.read() };
    
    if let Ok(Some(key_event)) = keyboard.add_byte(scancode) {
        if let Some(key) = keyboard.process_keyevent(key_event) {
            match key {
                DecodedKey::Unicode(character) => { crate::shell::handle_char(character); },
                DecodedKey::RawKey(k) => { 
                    use pc_keyboard::KeyCode;
                    if k == KeyCode::F1 { crate::shell::handle_key(0x3B); }
                },
            }
        }
    }
    unsafe { PICS.lock().notify_end_of_interrupt(PIC_1_OFFSET + 1); }
}

extern "x86-interrupt" fn mouse_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;
    let mut port = Port::<u8>::new(0x60);
    let packet_byte = unsafe { port.read() };
    crate::mouse::handle_interrupt(packet_byte);
    unsafe { PICS.lock().notify_end_of_interrupt(PIC_2_OFFSET + 4); }
}

extern "x86-interrupt" fn breakpoint_handler(_sf: InterruptStackFrame) {}

extern "x86-interrupt" fn page_fault_handler(sf: InterruptStackFrame, ec: PageFaultErrorCode) { 
    use x86_64::registers::control::Cr2;
    let fault_addr = Cr2::read();
    unsafe {
        if let Some(painter) = &mut crate::SCREEN_PAINTER {
            use crate::gui::Color;
            painter.clear(Color::RED);
            painter.draw_string(20, 20, "PAGE FAULT DETECTED", Color::WHITE);
            use alloc::format;
            let msg1 = format!("IP: {:#x} | Code: {:?}", sf.instruction_pointer.as_u64(), ec);
            let msg2 = format!("FAULT ADDR (CR2): {:#x}", fault_addr.as_u64());
            painter.draw_string(20, 40, &msg1, Color::WHITE);
            painter.draw_string(20, 60, &msg2, Color::WHITE);
        }
    }
    loop {} 
}

extern "x86-interrupt" fn double_fault_handler(_sf: InterruptStackFrame, _err: u64) -> ! {
    loop {}
}