use x86_64::structures::idt::{InterruptDescriptorTable, InterruptStackFrame, PageFaultErrorCode};
use lazy_static::lazy_static;
use pic8259::ChainedPics;
use spin::Mutex;
use crate::gdt;

pub const PIC_1_OFFSET: u8 = 32;
pub const PIC_2_OFFSET: u8 = PIC_1_OFFSET + 8;

pub static PICS: Mutex<ChainedPics> =
    Mutex::new(unsafe { ChainedPics::new(PIC_1_OFFSET, PIC_2_OFFSET) });

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
        
        // MOUSE INTERRUPT (IRQ 12 -> Offset 32 + 12 = 44)
        idt[PIC_1_OFFSET as usize + 12].set_handler_fn(mouse_interrupt_handler);
        
        idt
    };
}

pub fn init_idt() {
    IDT.load();
    unsafe {
        let mut pics = PICS.lock();
        pics.initialize();
        
        // --- CRITICAL FIX FOR MOUSE ---
        // Master PIC: 0b1111_1000 
        //   - Bit 0 (IRQ0 Timer) = 0 (Enabled)
        //   - Bit 1 (IRQ1 Keybd) = 0 (Enabled)
        //   - Bit 2 (IRQ2 Casc)  = 0 (Enabled) -> REQUIRED to hear the Slave PIC!
        //
        // Slave PIC:  0b1110_1111
        //   - Bit 4 (IRQ12 Mouse)= 0 (Enabled)
        pics.write_masks(0b1111_1000, 0b1110_1111);
    }
}

extern "x86-interrupt" fn timer_interrupt_handler(_stack_frame: InterruptStackFrame) {
    unsafe { PICS.lock().notify_end_of_interrupt(PIC_1_OFFSET); }
}

extern "x86-interrupt" fn keyboard_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use pc_keyboard::{layouts, DecodedKey, HandleControl, Keyboard, ScancodeSet1};
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
                DecodedKey::Unicode(character) => { crate::shell::handle_keyboard(character); },
                _ => {},
            }
        }
    }
    unsafe { PICS.lock().notify_end_of_interrupt(PIC_1_OFFSET + 1); }
}

// NEW: Mouse Handler
extern "x86-interrupt" fn mouse_interrupt_handler(_stack_frame: InterruptStackFrame) {
    use x86_64::instructions::port::Port;
    let mut port = Port::new(0x60);
    let packet: u8 = unsafe { port.read() };
    
    crate::mouse::MOUSE.lock().process_packet(packet);
    
    // Notify BOTH PICs because Mouse is on the Slave
    unsafe { 
        PICS.lock().notify_end_of_interrupt(PIC_1_OFFSET + 12); 
    }
}

extern "x86-interrupt" fn breakpoint_handler(_sf: InterruptStackFrame) {}
extern "x86-interrupt" fn page_fault_handler(_sf: InterruptStackFrame, _err: PageFaultErrorCode) { loop {} }
extern "x86-interrupt" fn double_fault_handler(_sf: InterruptStackFrame, _err: u64) -> ! { loop {} }