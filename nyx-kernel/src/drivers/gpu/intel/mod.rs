// src/drivers/gpu/intel/mod.rs

use core::ptr::{read_volatile, write_volatile};
use core::sync::atomic::{AtomicU64, Ordering};
use spin::Mutex;

pub static INTEL_GPU: Mutex<Option<IntelGpuDriver>> = Mutex::new(None);
pub static BACKBUFFER_PHYS_ADDR: AtomicU64 = AtomicU64::new(0);

// --- PRM Verified Registers ---
pub const BLT_RING_TAIL: u32 = 0x22030;
pub const BLT_RING_HEAD: u32 = 0x22034;
const BLT_RING_START: u32 = 0x22038;
const BLT_RING_CTL: u32 = 0x2203C;
const FORCEWAKE_BLT: u32 = 0xA188;
const FORCEWAKE_ACK_BLT: u32 = 0x130044;
const PIPEA_STAT: u32 = 0x70024;

// --- Display Plane Surface Registers (Gen9+) ---
const PLANE_SURF_1_A: u32 = 0x701C4;
const PLANE_SURF_1_B: u32 = 0x711C4;
const PLANE_SURF_1_C: u32 = 0x721C4;

const VGA_CONTROL: u32 = 0x41000;
const VGA_DISP_DISABLE: u32 = 1 << 31;

#[derive(Debug, PartialEq)]
pub enum GpuGeneration {
    Gen9, Gen11, Gen12, Unsupported,
}

#[derive(Debug)]
pub enum GpuHangError {
    RingBufferFull,
    EngineHung,
}

pub struct IntelGpuDriver {
    pub mmio_base: u64,
    pub device_id: u16,
    pub generation: GpuGeneration,
    pub ring_virt_addr: Option<u64>,
    pub active_gva: u32,
    pub pci_bus: u8,
    pub pci_device: u8,
    pub pci_func: u8,
    pub stolen_memory_base: u64,
    pub stolen_memory_size: u64,
    pub backbuffer_phys: u64,
    pub backbuffer_size: u64,
    pub fence_phys: u64,
    pub fence_virt: *mut u32,
    pub next_fence_val: u32,
    pub current_fence_target: u32,
}

unsafe impl Send for IntelGpuDriver {}
unsafe impl Sync for IntelGpuDriver {}

impl IntelGpuDriver {
    pub fn probe(device_id: u16) -> GpuGeneration {
        match device_id {
            0x3185 | 0x3E9B | 0x5917 | 0x5916 | 0x9BC4 => GpuGeneration::Gen9, 
            0x8A56 => GpuGeneration::Gen11,
            0x9A49 => GpuGeneration::Gen12,
            _ => GpuGeneration::Unsupported,
        }
    }

    pub fn new(mmio_base: u64, device_id: u16, bus: u8, device: u8, func: u8) -> Self {
        let generation = Self::probe(device_id);
        Self { 
            mmio_base, device_id, generation, ring_virt_addr: None, active_gva: 0, 
            pci_bus: bus, pci_device: device, pci_func: func, 
            stolen_memory_base: 0, stolen_memory_size: 0,
            backbuffer_phys: 0, backbuffer_size: 0,
            fence_phys: 0, fence_virt: core::ptr::null_mut(),
            next_fence_val: 0, current_fence_target: 0
        }
    }

    pub unsafe fn read_reg(&self, offset: u32) -> u32 {
        read_volatile((self.mmio_base + offset as u64) as *const u32)
    }

    pub unsafe fn write_reg(&self, offset: u32, value: u32) {
        write_volatile((self.mmio_base + offset as u64) as *mut u32, value);
    }

    pub unsafe fn wake_up_gpu(&self) {
        self.write_reg(FORCEWAKE_BLT, 0x00010001);
        while (self.read_reg(FORCEWAKE_ACK_BLT) & 1) == 0 {
            core::hint::spin_loop(); 
        }
    }

    pub unsafe fn map_ggtt_page(&self, gpu_page_number: u32, phys_ram_addr: u64, coherent: bool) {
        // Gen8+ GGTT PTEs start at offset 0x800000 (8MB)
        let gtt_offset = 0x800000 + (gpu_page_number as u64 * 8);
        let gtt_ptr = (self.mmio_base + gtt_offset) as *mut u64;
        
        let flags = if coherent { 0x07 } else { 0x03 };
        let pte_value = (phys_ram_addr & 0xFFFFFFFFFFFFF000) | flags; 
        
        core::ptr::write_volatile(gtt_ptr, pte_value);
        let _ = core::ptr::read_volatile(gtt_ptr); 
    }

    pub unsafe fn get_active_framebuffer_gva(&self) -> u32 {
        let surf_a = self.read_reg(PLANE_SURF_1_A) & !0xFFF;
        let surf_b = self.read_reg(PLANE_SURF_1_B) & !0xFFF;
        let surf_c = self.read_reg(PLANE_SURF_1_C) & !0xFFF;
        
        if surf_a != 0 { return surf_a; }
        if surf_b != 0 { return surf_b; }
        if surf_c != 0 { return surf_c; }
        0 
    }

    pub unsafe fn wait_for_vsync(&self) {
        self.write_reg(PIPEA_STAT, 1 << 1);
        let mut timeout = 0;
        while (self.read_reg(PIPEA_STAT) & (1 << 1)) == 0 && timeout < 150_000 {
            core::hint::spin_loop();
            timeout += 1;
        }
    }

    pub unsafe fn fill_rect(&mut self, dest_gpu_addr: u32, x: u32, y: u32, w: u32, h: u32, color: u32, pitch: u32) -> Result<(), GpuHangError> {
        let color_blit: [u32; 8] = [
            (0x2 << 29) | (0x50 << 22) | (1 << 21) | (1 << 20) | 0x05, 
            (3 << 24) | (0xF0 << 16) | pitch,  
            (y << 16) | x,                     
            ((y + h) << 16) | (x + w),         
            dest_gpu_addr,                     
            0x0,                               
            color,                             
            0x0 
        ];
        self.submit_command(&color_blit)?;

        let flush_cmd: [u32; 4] = [(0x26 << 23) | 2, 0x0, 0x0, 0x0];
        self.submit_command(&flush_cmd)?;
        Ok(())
    }

    pub unsafe fn copy_rect(&mut self, 
        src_x: u32, src_y: u32, src_pitch: u32, src_gpu_addr: u32,
        dst_x: u32, dst_y: u32, dst_pitch: u32, dst_gpu_addr: u32,
        w: u32, h: u32
    ) -> Result<(), GpuHangError> {
        let copy_blit: [u32; 10] = [
            (0x2 << 29) | (0x53 << 22) | (1 << 21) | (1 << 20) | 0x08, 
            (3 << 24) | (0xCC << 16) | dst_pitch, 
            (dst_y << 16) | dst_x,             
            ((dst_y + h) << 16) | (dst_x + w), 
            dst_gpu_addr,
            0x0,       
            (src_y << 16) | src_x,             
            src_pitch,                         
            src_gpu_addr,
            0x0        
        ];
        self.submit_command(&copy_blit)?;

        let flush_cmd: [u32; 4] = [(0x26 << 23) | 2, 0x0, 0x0, 0x0];
        self.submit_command(&flush_cmd)?;
        Ok(())
    }

    pub unsafe fn submit_command(&mut self, dwords: &[u32]) -> Result<(), GpuHangError> {
        if let Some(ring_ptr) = self.ring_virt_addr {
            let ring = ring_ptr as *mut u32;
            let mut timeout = 0;
            
            loop {
                let head_idx = self.read_reg(BLT_RING_HEAD) / 4;
                let tail_idx = self.read_reg(BLT_RING_TAIL) / 4;
                
                let free_space = if head_idx > tail_idx {
                    head_idx - tail_idx - 1
                } else {
                    1024 - tail_idx + head_idx - 1
                };
                
                if free_space >= dwords.len() as u32 { break; }
                
                if timeout > 1_000_000 {
                    crate::serial_println!("[INTEL GPU] FATAL: Ring buffer full / GPU Hang detected!");
                    return Err(GpuHangError::RingBufferFull);
                }
                core::hint::spin_loop();
                timeout += 1;
            }

            let mut tail_idx = self.read_reg(BLT_RING_TAIL) / 4;
            for &dw in dwords {
                let ptr = ring.add(tail_idx as usize);
                ptr.write_volatile(dw);
                
                // Explicitly push the CPU Cache line to RAM
                core::arch::asm!("clflush [{}]", in(reg) ptr, options(nostack, preserves_flags));
                
                tail_idx += 1;
                if tail_idx >= 1024 { tail_idx = 0; } 
            }

            core::arch::asm!("mfence", options(nostack, preserves_flags));
            self.write_reg(BLT_RING_TAIL, tail_idx * 4);
        }
        Ok(())
    }

    pub unsafe fn wait_for_idle(&self) -> Result<(), GpuHangError> {
        // Wait using fence instead of HEAD == TAIL (which only tracks command fetch, not execution completion)
        if self.current_fence_target != 0 {
            self.wait_fence(self.current_fence_target);
        }
        Ok(())
    }

    pub unsafe fn submit_fence(&mut self) -> u32 {
        self.next_fence_val = self.next_fence_val.wrapping_add(1);
        let val = self.next_fence_val;
        
        let fence_cmd: [u32; 4] = [
            (0x20 << 23) | (1 << 22) | 0x02, // MI_STORE_DATA_IMM + Use GTT + Length 2 (4 Dwords)
            0x1500_0000,                    // GVA low
            0x0,                            // GVA high
            val,                            // Data
        ];
        let _ = self.submit_command(&fence_cmd);
        self.current_fence_target = val;
        val
    }

    pub unsafe fn wait_fence(&self, val: u32) {
        let mut timeout = 0;
        while (core::ptr::read_volatile(self.fence_virt) as i32).wrapping_sub(val as i32) < 0 {
            if timeout > 50_000_000 {
                crate::serial_println!("[INTEL GPU] FATAL: Fence wait timeout! GPU Hung? (Expected {}, Got {})", val, core::ptr::read_volatile(self.fence_virt));
                break;
            }
            core::hint::spin_loop();
            timeout += 1;
        }
    }

    pub unsafe fn disable_vga(&mut self) {
        crate::serial_println!("[INTEL GPU] Disabling legacy VGA plane...");
        use x86_64::instructions::port::Port;
        
        // 1. Sequencer register SR01 - turn off screen
        let mut sr_index: Port<u8> = Port::new(0x3C4);
        let mut sr_data: Port<u8> = Port::new(0x3C5);
        sr_index.write(0x01);
        let mut sr01 = sr_data.read();
        sr01 |= 1 << 5; // Screen off bit
        sr_data.write(sr01);
        
        // Brief wait
        for _ in 0..100_000 { core::hint::spin_loop(); }

        // 2. Disable VGA routing in MMIO
        let mut vga_ctl = self.read_reg(VGA_CONTROL);
        vga_ctl |= VGA_DISP_DISABLE;
        self.write_reg(VGA_CONTROL, vga_ctl);
        crate::serial_println!("[INTEL GPU] VGA disabled. Display engine ready for handoff.");
    }

    pub fn initialize(&mut self) {
        crate::serial_println!("[INTEL GPU] Initializing {:?} Graphics Engine (Device ID: {:#06x})...", self.generation, self.device_id);
        
        // 1. Detect Stolen Memory (BDSM register at PCI offset 0x5C)
        let bdsm = crate::pci::PciDriver::read_config(self.pci_bus, self.pci_device, self.pci_func, 0x5C);
        self.stolen_memory_base = (bdsm & 0xFFF00000) as u64;
        
        // WaSkipStolenMemoryFirstPage: The hardware might sporadically write to the first page of stolen memory.
        if self.stolen_memory_base != 0 {
            self.stolen_memory_base += 4096;
            crate::serial_println!("[INTEL GPU] Stolen Memory Base (Workaround applied): {:#010x}", self.stolen_memory_base);
        }

        unsafe {
            // Disable legacy VGA to take over the display
            self.disable_vga();

            // 2. Allocate a safe normal RAM page for the Ring Buffer
            let ring_frame = crate::memory::allocate_frame().unwrap();
            let ring_phys = ring_frame.start_address().as_u64();
            self.ring_virt_addr = Some(crate::memory::phys_to_virt(ring_phys).unwrap());

            // Map it high in the GGTT, safely away from 0x0 (The Null Trap)
            let ring_gva: u32 = 0x1000_0000;
            self.map_ggtt_page(ring_gva / 4096, ring_phys, true); 

            self.wake_up_gpu();
            self.write_reg(BLT_RING_CTL, 0); 
            self.write_reg(BLT_RING_START, ring_gva); 
            self.write_reg(BLT_RING_HEAD, 0);
            self.write_reg(BLT_RING_TAIL, 0);
            self.write_reg(BLT_RING_CTL, 0x00000001); 

            let ctl = self.read_reg(BLT_RING_CTL);
            crate::serial_println!("[INTEL GPU] Ring CTL Readback: {:#010x}", ctl);

            // 3. Allocate backbuffer in contiguous RAM
            let bb_size = if let Some(p) = &crate::gui::SCREEN_PAINTER {
                (p.info.stride * p.info.height * p.info.bytes_per_pixel) as u64
            } else {
                1920 * 1080 * 4
            };
            self.backbuffer_size = bb_size;

            let pages_needed = (bb_size + 4095) / 4096;
            if let Some(bb_frame) = crate::memory::allocate_contiguous(pages_needed as usize, 4096, true) {
                let bb_phys = bb_frame.start_address().as_u64();
                self.backbuffer_phys = bb_phys;
                let phys_ram_addr = bb_frame.start_address().as_u64();
                self.backbuffer_phys = phys_ram_addr;
                
                // Map the backbuffer to GGTT address 0x1400_0000
                for i in 0..pages_needed {
                    self.map_ggtt_page((0x1400_0000 / 4096) + i as u32, phys_ram_addr + (i * 4096), true);
                }
                
                crate::serial_println!("[INTEL GPU] Allocated hardware backbuffer at Phys {:#x} (mapped to GVA 0x14000000)", phys_ram_addr);
            } else {
                crate::serial_println!("[INTEL GPU] ERROR: Failed to allocate contiguous memory for backbuffer!");
            }

            // 4. Allocate a page for fence synchronization
            if let Some(fence_frame) = crate::memory::allocate_contiguous(1, 4096, true) {
                let f_phys = fence_frame.start_address().as_u64();
                self.fence_phys = f_phys;
                self.fence_virt = crate::memory::phys_to_virt(f_phys).unwrap() as *mut u32;
                self.fence_virt.write_volatile(0);
                
                // Map the fence page to 0x1500_0000 in the GGTT
                self.map_ggtt_page(0x1500_0000 / 4096, f_phys, true);
                crate::serial_println!("[INTEL GPU] Allocated hardware fence page at Phys {:#x} (mapped to GVA 0x15000000)", f_phys);
            }
        }
    }

    pub fn test_blitter(&mut self) {
        crate::serial_println!("[INTEL GPU] Executing Ring 0 Blitter Test (XY_COLOR_BLT)...");
        
        unsafe {
            let target_frame = crate::memory::allocate_frame().unwrap();
            let target_phys = target_frame.start_address().as_u64();
            let target_virt = crate::memory::phys_to_virt(target_phys).unwrap() as *mut u32;

            let target_gva: u32 = 0x3000_0000;
            self.map_ggtt_page(target_gva / 4096, target_phys, true);

            // Verify GGTT entry was written
            let gtt_offset = 0x800000 + ((target_gva / 4096) as u64 * 8);
            let pte_readback = core::ptr::read_volatile((self.mmio_base + gtt_offset) as *const u64);
            crate::serial_println!("[INTEL GPU] PTE at 0x{:x} (target_gva 0x{:x}): {:#x} (Phys: {:#x})", gtt_offset, target_gva, pte_readback, target_phys);

            core::ptr::write_bytes(target_virt as *mut u8, 0, 4096);
            
            // Flush the whole page from CPU cache
            for i in (0..4096).step_by(64) {
                core::arch::asm!("clflush [{}]", in(reg) (target_virt as usize + i), options(nostack, preserves_flags));
            }
            core::arch::asm!("mfence", options(nostack, preserves_flags));
            
            // 1. Test standard MI_STORE_DATA_IMM to offset 0x00 to prove Ring Buffer works
            let mi_store: [u32; 4] = [
                (0x20 << 23) | (1 << 22) | 0x02, // MI_STORE_DATA_IMM
                target_gva,                      // Safe Destination
                0x0,                             
                0x1337BEEF,                      // Magic Data
            ];
            self.submit_command(&mi_store).unwrap();
            
            // 2. Test XY_COLOR_BLT to offset 0x40 (no overlap with offset 0x00)
            let pitch = 64; 
            match self.fill_rect(target_gva + 0x40, 0, 0, 10, 10, 0xAABBCCDD, pitch) {
                Ok(_) => crate::serial_println!("[INTEL GPU] XY_COLOR_BLT submitted."),
                Err(_) => crate::serial_println!("[INTEL GPU] Blitter Error."),
            }
            
            self.wait_for_idle().unwrap();
            
            // Flush the whole page again before reading
            for i in (0..4096).step_by(64) {
                core::arch::asm!("clflush [{}]", in(reg) (target_virt as usize + i), options(nostack, preserves_flags));
            }
            core::arch::asm!("mfence", options(nostack, preserves_flags));
            
            let check_store = target_virt.read_volatile(); // offset 0x00 bytes
            let check_blit = target_virt.add(16).read_volatile(); // offset 0x40 bytes = 16 dwords
            
            let fault_reg = self.read_reg(0x4094);
            if fault_reg != 0 {
                crate::serial_println!("[INTEL GPU] WARNING: FAULT_REG (0x4094) = {:#010x}", fault_reg);
            }

            if check_store == 0x1337BEEF {
                crate::serial_println!("[INTEL GPU] -> MI_STORE_DATA_IMM SUCCESS (Ring Buffer works!)");
            } else {
                crate::serial_println!("[INTEL GPU] -> MI_STORE_DATA_IMM FAILED: {:#010x}", check_store);
            }
            
            if check_blit == 0xAABBCCDD {
                crate::serial_println!("[INTEL GPU] -> XY_COLOR_BLT SUCCESS (Blitter works!)");
            } else {
                crate::serial_println!("[INTEL GPU] -> XY_COLOR_BLT FAILED: {:#010x}", check_blit);
            }
        }
    }

    pub unsafe fn test_screen_blit(&mut self) {
        crate::serial_println!("[INTEL GPU] Dumping display plane registers to find active configuration...");
        
        // Dump Pipe A, B, C plane registers
        let pipes = [("A", 0x70000), ("B", 0x71000), ("C", 0x72000)];
        let planes = [1, 2, 3];
        
        let mut active_gva = None;
        let mut active_pipe = "None";
        let mut active_plane = 0;
        
        for &(pipe_name, pipe_offset) in &pipes {
            for &plane in &planes {
                let ctl_reg = pipe_offset + 0x180 + (plane - 1) * 0x100;
                let surf_reg = pipe_offset + 0x1C4 + (plane - 1) * 0x100;
                
                let ctl_val = self.read_reg(ctl_reg);
                let surf_val = self.read_reg(surf_reg);
                
                if (ctl_val & (1 << 31)) != 0 {
                    crate::serial_println!("[INTEL GPU] Active Pipe {} Plane {}: CTL={:#010x}, SURF={:#010x}", pipe_name, plane, ctl_val, surf_val);
                    active_gva = Some(surf_val & !0xFFF);
                    active_pipe = pipe_name;
                    active_plane = plane;
                    
                    // Read additional properties
                    let stride_reg = pipe_offset + 0x188 + (plane - 1) * 0x100;
                    let size_reg = pipe_offset + 0x190 + (plane - 1) * 0x100;
                    let stride_val = self.read_reg(stride_reg);
                    let size_val = self.read_reg(size_reg);
                    
                    let width = (size_val & 0x1FFF) + 1;
                    let height = ((size_val >> 16) & 0x1FFF) + 1;
                    crate::serial_println!("[INTEL GPU] Properties -> Stride Reg: {:#x}, Size Reg: {:#x} (Width: {}, Height: {})", stride_val, size_val, width, height);
                }
            }
        }

        if let Some(gva) = active_gva {
            crate::serial_println!("[INTEL GPU] Hijacking active screen at GVA {:#010x}! Drawing 300x300 Red Square...", gva);
            // Let's draw at X=100, Y=100. Width=300, Height=300. Color = Red (0xFFFF0000).
            // Stride unit on Gen9 linear is 64 bytes. Let's calculate pitch from the Stride register.
            // If Stride register is 120 (for 1920 wide screen * 4 = 7680 bytes / 64 = 120), we use it.
            // Let's read Stride register again.
            let pipe_offset = match active_pipe {
                "B" => 0x71000,
                "C" => 0x72000,
                _ => 0x70000,
            };
            let stride_val = self.read_reg(pipe_offset + 0x188 + (active_plane - 1) * 0x100);
            
            // Stride value is in 64-byte units for linear, or we can just fall back to width * 4.
            // Let's use stride_val * 64 if it's not 0, otherwise width * 4.
            let size_val = self.read_reg(pipe_offset + 0x190 + (active_plane - 1) * 0x100);
            let width = (size_val & 0x1FFF) + 1;
            let pitch_bytes = if stride_val != 0 { stride_val * 64 } else { width * 4 };
            
            crate::serial_println!("[INTEL GPU] Calculated pitch: {} bytes (Stride register: {})", pitch_bytes, stride_val);
            
            match self.fill_rect(gva, 100, 100, 300, 300, 0xFFFF0000, pitch_bytes) {
                Ok(_) => crate::serial_println!("[INTEL GPU] Screen Blit submitted successfully! Check monitor!"),
                Err(_) => crate::serial_println!("[INTEL GPU] Screen Blit failed!"),
            }
        } else {
            crate::serial_println!("[INTEL GPU] No active display plane detected (Enable bit 31 not set on any plane).");
        }
    }
}