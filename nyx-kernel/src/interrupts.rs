use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use lazy_static::lazy_static;
use pic8259::ChainedPics;
use spin::Mutex;
use crate::gdt;
use pc_keyboard::{layouts, DecodedKey, HandleControl, Keyboard, ScancodeSet1}; 
use crate::gui::Painter; 
use core::arch::global_asm;

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

// --- ASSEMBLY WRAPPER ---
// Captures registers before passing control to Rust.
global_asm!(r#"
.global syscall_asm_wrapper
syscall_asm_wrapper:
    push rax
    push rdi
    push rsi
    push rdx
    push rcx
    push r8
    push r9
    
    // Pass Stack Pointer (RSP) to Rust as the first argument (RDI)
    mov rdi, rsp

    // Call the Rust function
    call syscall_rust_dispatcher

    pop r9
    pop r8
    pop rcx
    pop rdx
    pop rsi
    pop rdi
    pop rax
    iretq
"#);

extern "C" {
    fn syscall_asm_wrapper();
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
        
        // --- SYSCALL INTERRUPT (0x80) ---
        unsafe {
            idt[0x80]
                .set_handler_addr(x86_64::VirtAddr::new(syscall_asm_wrapper as u64))
                .set_privilege_level(x86_64::PrivilegeLevel::Ring3);
        }
        
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

// --- HANDLERS ---

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    crate::time::tick();
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
                DecodedKey::RawKey(key_code) => { crate::shell::handle_key(key_code); },
            }
        }
    }
    unsafe { PICS.lock().notify_end_of_interrupt(PIC_1_OFFSET + 1); }
}

extern "x86-interrupt" fn mouse_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;
    let mut port = Port::new(0x60);
    let packet: u8 = unsafe { port.read() }; 
    crate::mouse::handle_interrupt(packet);
    unsafe { PICS.lock().notify_end_of_interrupt(PIC_2_OFFSET + 4); }
}

// --- SYSCALL DISPATCHER ---
#[repr(C)]
struct SyscallRegisters {
    r9: u64, r8: u64, rcx: u64, rdx: u64,
    rsi: u64, rdi: u64, rax: u64,
}

#[no_mangle]
extern "C" fn syscall_rust_dispatcher(stack_ptr: *mut SyscallRegisters) {
    use crate::gui::{Color, Rect};
    
    let regs = unsafe { &*stack_ptr };
    let syscall_id = regs.rax; // RAX = Command
    let arg1 = regs.rdi;       // RDI = Data

    unsafe {
        if let Some(painter) = &mut crate::SCREEN_PAINTER {
            match syscall_id {
                0 => { // SYSCALL 0: EXIT PROCESS
                    painter.draw_rect(Rect::new(100, 400, 600, 50), Color::RED);
                    painter.draw_string(110, 410, "PROCESS FINISHED: sys_exit(0) called.", Color::WHITE);
                    painter.draw_string(110, 430, "System Halted safely.", Color::WHITE);
                    // Halt the CPU permanently (since we have no other tasks yet)
                    loop { x86_64::instructions::hlt(); }
                },
                1 => { // SYSCALL 1: PRINT CHARACTER
                    let char_to_print = arg1 as u8 as char;
                    
                    // Draw Blue Box
                    painter.draw_rect(Rect::new(100, 300, 600, 50), Color::DARK_BLUE);
                    use alloc::format;
                    let msg = format!("USER OUTPUT: '{}'", char_to_print);
                    painter.draw_string(110, 310, &msg, Color::WHITE);
                },
                _ => {
                     painter.draw_rect(Rect::new(100, 300, 600, 50), Color::RED);
                     painter.draw_string(110, 310, "UNKNOWN SYSCALL", Color::WHITE);
                }
            }
        }
    }
}

extern "x86-interrupt" fn breakpoint_handler(_sf: InterruptStackFrame) {}

extern "x86-interrupt" fn page_fault_handler(sf: InterruptStackFrame, ec: PageFaultErrorCode) { 
    unsafe {
        if let Some(painter) = &mut crate::SCREEN_PAINTER {
            use crate::gui::Color;
            painter.clear(Color::RED);
            painter.draw_string(20, 20, "PAGE FAULT", Color::WHITE);
            use alloc::format;
            let msg = format!("IP: {:#x} | Code: {:?}", sf.instruction_pointer.as_u64(), ec);
            painter.draw_string(20, 40, &msg, Color::WHITE);
        }
    }
    loop {} 
}

extern "x86-interrupt" fn double_fault_handler(sf: InterruptStackFrame, _err: u64) -> ! {
    unsafe {
        if let Some(painter) = &mut crate::SCREEN_PAINTER {
            use crate::gui::Color;
            painter.clear(Color::RED);
            painter.draw_string(20, 20, "DOUBLE FAULT", Color::WHITE);
            use alloc::format;
            let msg = format!("IP: {:#x}", sf.instruction_pointer.as_u64());
            painter.draw_string(20, 40, &msg, Color::WHITE);
        }
    }
    loop {}
}